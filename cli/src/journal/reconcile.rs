use super::*;

type CentralStatus<'a> = dyn Fn(&str) -> Result<Value> + 'a;

pub(crate) fn record_central_id(root: &Path, module: &str, version: &str, id: &str) -> Result<()> {
    crate::central::validate_id(id)?;
    let (mut record, oid) = read_reservation(root, module, version, "maven-central", "")?;
    ensure!(
        record["phase"] == "reserved-before-upload",
        "Cannot attach a receipt to a confirmed journal"
    );
    ensure!(
        record["source"] == git_text(root, &["rev-parse", "HEAD"])?,
        "Receipt checkout differs from reserved source"
    );
    let marker = attempt("uploading", module, version, "maven-central");
    let marker_oid = remote_oid(root, &marker)?
        .context("Missing upload guard; cannot record Central receipt")?;
    ensure!(
        peel(root, &marker_oid)? == field(&record, "source")?,
        "Upload marker source mismatch"
    );
    if let Some(existing) = record.get("central_deployment_id") {
        ensure!(
            existing == id,
            "Conflicting Central deployment IDs; preserve the journal"
        );
        return Ok(());
    }
    record["central_deployment_id"] = json!(id);
    let raw = git_text(root, &["cat-file", "tag", &oid])?;
    let (header, _) = raw
        .split_once("\n\n")
        .context("Invalid annotated journal")?;
    let payload = format!(
        "{header}\n\n{}{}\n",
        serde_json::to_string(&record)?,
        tag_nonce()?
    );
    let mut child = Command::new("git")
        .arg("mktag")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .context("Missing Git input")?
        .write_all(payload.as_bytes())?;
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "Could not create Central receipt journal"
    );
    let new_oid = std::str::from_utf8(&output.stdout)?.trim();
    let reference = refname(&attempt("pending", module, version, "maven-central"));
    git_text(
        root,
        &[
            "push",
            "--atomic",
            &format!("--force-with-lease={reference}:{oid}"),
            "origin",
            &format!("{new_oid}:{reference}"),
        ],
    )?;
    git_text(root, &["update-ref", &reference, new_oid, &oid])?;
    Ok(())
}

pub(crate) fn track_central_upload(root: &Path, module: &str, version: &str) -> Result<()> {
    use std::io::BufRead;
    let mut receipt = None;
    let mut failure = None;
    // Drain even after a persistence failure: tee retains the complete Gradle log
    // and pipefail still reports both the upload and tracker result accurately.
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        println!("{line}");
        let result = (|| -> Result<()> {
            if let Some(id) = crate::central::upload_id(&line)? {
                if let Some(previous) = &receipt {
                    ensure!(
                        previous == &id,
                        "Multiple deployment IDs in a single-module upload"
                    );
                }
                record_central_id(root, module, version, &id)?;
                receipt = Some(id);
            }
            Ok(())
        })();
        if let Err(error) = result {
            if failure.is_none() {
                failure = Some(error);
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    ensure!(receipt.is_some(),"No Central upload receipt captured; preserve any upload-started journal and inspect the retained log");
    Ok(())
}

enum Decision {
    Failed(String),
    Published(Value),
    Active,
}

fn no_confirmation(root: &Path, record: &Value, destination: &str) -> Result<()> {
    let module = field(record, "module")?;
    let version = field(record, "version")?;
    if let Some(latest) = current(root, module, destination)? {
        ensure!(
            latest["version"] != version,
            "Version has a confirmed release record; never archive it as failed"
        );
    }
    for filename in [
        format!("{version}-{destination}.json"),
        format!("{version}.json"),
    ] {
        if let Some(history) = remote_file(
            root,
            &format!("{}/{module}/{filename}", crate::docs::HISTORY),
        )? {
            ensure!(
                history["destinations"]
                    .as_object()
                    .context("Malformed release history")?
                    .get(destination)
                    .is_none(),
                "Version has confirmed release history; never archive it as failed"
            );
        }
    }
    Ok(())
}

fn inspect(
    root: &Path,
    record: &Value,
    marker: Option<&str>,
    fetch: &Fetch<'_>,
    central: &CentralStatus<'_>,
) -> Result<Decision> {
    let destination = record_destination(record)?;
    let group = field(record, "group")?;
    let artifact = field(record, "artifact")?;
    let version = field(record, "version")?;
    if destination == "jitpack" {
        ensure!(
            marker.is_none(),
            "Unexpected JitPack upload marker; preserve the journal"
        );
        let consumer = field(record, "consumer_version")?;
        let bytes = fetch(
            &format!("{JITPACK}/api/builds/{group}/{artifact}/{consumer}"),
            false,
        )?
        .context("Missing JitPack status")?;
        let status: Value = serde_json::from_slice(&bytes)?;
        ensure!(
            status["commit"] == record["source"],
            "JitPack status source differs from reservation"
        );
        match status["status"]
            .as_str()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("error" | "failed") => {
                verify_failed_jitpack(record, fetch)?;
                return Ok(Decision::Failed(
                    "JitPack terminal failure and no public artifacts".into(),
                ));
            }
            Some("ok") => {
                return Ok(Decision::Published(crate::confirmation::confirmed_record(
                    root, record, fetch,
                )?))
            }
            Some("queued" | "pending" | "building" | "running") => return Ok(Decision::Active),
            _ => anyhow::bail!("JitPack status is unknown; preserve the journal"),
        }
    }
    ensure!(
        destination == "maven-central",
        "Unsupported reconciliation destination"
    );
    let base = artifact_base(CENTRAL, group, artifact, version);
    // Public immutable bytes are authoritative, even when the runner lost its
    // upload receipt. Partial/mismatched publication fails full verification.
    if artifacts_exist(&base, fetch)? {
        return Ok(Decision::Published(crate::confirmation::confirmed_record(
            root, record, fetch,
        )?));
    }
    if marker.is_none() && record.get("central_deployment_id").is_none() {
        ensure!(
            record["upload_protocol"] == "guard-v1",
            "Legacy reservation lacks evidence that upload never started"
        );
        return Ok(Decision::Failed(
            "Upload never passed the atomic guard; reservation fenced".into(),
        ));
    }
    let id=field(record,"central_deployment_id").context("Upload may have started but its Central deployment ID is missing; inspect the retained log")?;
    let check = || crate::central::classify(&central(id)?, id, group, artifact, version);
    match check()? {
        crate::central::State::Active => Ok(Decision::Active),
        crate::central::State::Published => {
            anyhow::bail!("Central reports PUBLISHED; public artifacts have not propagated yet")
        }
        crate::central::State::Failed => {
            ensure!(
                !artifacts_exist(&base, fetch)?,
                "Public artifacts appeared during failure verification"
            );
            ensure!(
                check()? == crate::central::State::Failed,
                "Central deployment state changed during verification"
            );
            Ok(Decision::Failed(
                "Central terminal failure and no public artifacts".into(),
            ))
        }
    }
}

fn finalize_isolated(root: &Path, record: &Value) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let worktree = temp.path().join("finalize");
    let path = worktree.to_str().context("Invalid worktree path")?;
    let output = Command::new("git")
        .args(["worktree", "add", "--detach", path, "origin/main"])
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .current_dir(root)
        .output()?;
    ensure!(
        output.status.success(),
        "Could not create isolated finalization checkout: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = (|| {
        let module = field(record, "module")?;
        let destination = record_destination(record)?;
        let path = worktree.join(format!(
            "build/{}confirmed-{module}.json",
            scope(destination)
        ));
        fs::create_dir_all(path.parent().context("Missing output directory")?)?;
        fs::write(path, serde_json::to_vec_pretty(record)?)?;
        finalize(
            &worktree,
            module,
            field(record, "version")?,
            field(record, "source")?,
            destination,
        )
    })();
    let cleanup = git_text(root, &["worktree", "remove", "--force", path]);
    result?;
    cleanup?;
    Ok(())
}

fn reconcile_with(
    root: &Path,
    module: &str,
    destination: &str,
    versions: &[String],
    apply: bool,
    fetch: &Fetch<'_>,
    central: &CentralStatus<'_>,
) -> Result<Vec<ReconciledAttempt>> {
    coordinates(root, module, "0.1.0", destination, true)?;
    for version in versions {
        version_key(version)?;
    }
    let tags = remote_tags(root)?;
    let prefix = attempt("pending", module, "", destination);
    let marker_prefix = attempt("uploading", module, "", destination);
    let mut candidates: BTreeSet<String> = tags
        .keys()
        .filter_map(|name| {
            name.strip_prefix(&prefix)
                .or_else(|| name.strip_prefix(&marker_prefix))
        })
        .map(str::to_owned)
        .collect();
    candidates.extend(versions.iter().cloned());
    let mut report = Vec::new();
    for version in candidates
        .iter()
        .filter(|version| versions.is_empty() || versions.contains(version))
    {
        let pending = attempt("pending", module, version, destination);
        let marker = attempt("uploading", module, version, destination);
        if !tags.contains_key(&pending) && !tags.contains_key(&marker) {
            continue;
        }
        let result = (|| -> Result<(&'static str, String)> {
            version_key(version)?;
            let expected = tags
                .get(&pending)
                .context("Upload marker without reservation; manual review required")?;
            let (record, oid) =
                read_reservation(root, module, version, destination, scope(destination))?;
            ensure!(
                &oid == expected,
                "Pending journal changed during reconciliation"
            );
            ensure!(
                record["phase"] == "reserved-before-upload",
                "Journal is already confirmed"
            );
            verify_stable(root, &record)?;
            let marker_oid = tags.get(&marker);
            if let Some(marker_oid) = marker_oid {
                ensure!(
                    peel(root, marker_oid)? == field(&record, "source")?,
                    "Upload marker source differs"
                );
            }
            let decision = inspect(
                root,
                &record,
                marker_oid.map(String::as_str),
                fetch,
                central,
            )?;
            match decision {
                Decision::Active=>Ok(("active","Provider is still processing or awaiting manual publication; preserve the journal".into())),
                Decision::Failed(detail)=>{
                    no_confirmation(root,&record,destination)?;
                    verify_stable(root,&record)?;
                    ensure!(remote_oid(root,&marker)?.as_ref()==marker_oid,"Upload marker changed during reconciliation");
                    let archive=attempt("failed",module,version,destination);
                    ensure!(remote_oid(root,&archive)?.is_none(),"Failure archive already exists; preserve journal");
                    if apply {
                        let mut refs=vec![(pending.as_str(),oid.as_str())];
                        if let Some(marker_oid)=marker_oid {refs.push((marker.as_str(),marker_oid));}
                        archive_attempt(root,&archive,&refs)?;
                    }
                    Ok((if apply {"archived"}else{"eligible"},format!("{detail}; {pending} -> {archive} (object {oid})")))
                }
                Decision::Published(confirmed)=>{
                    ensure!(remote_oid(root,&pending)?.as_ref()==Some(&oid),"Journal changed after public verification");
                    if apply {finalize_isolated(root,&confirmed)?;}
                    Ok((if apply {"finalized"}else{"published"},"All public artifacts verified; finalize bookkeeping without upload".into()))
                }
            }
        })();
        report.push(match result {
            Ok((action, detail)) => ReconciledAttempt {
                version: version.clone(),
                action,
                detail,
            },
            Err(error) => ReconciledAttempt {
                version: version.clone(),
                action: "needs-review",
                detail: format!("{error:#}"),
            },
        });
    }
    Ok(report)
}

pub(crate) fn reconcile(
    root: &Path,
    module: &str,
    destination: &str,
    versions: &[String],
    apply: bool,
) -> Result<()> {
    fetch_tags(root)?;
    let url = match destination {
        "maven-central" => CENTRAL,
        "jitpack" => JITPACK,
        _ => anyhow::bail!("Unsupported reconciliation destination"),
    };
    let report = reconcile_with(
        root,
        module,
        destination,
        versions,
        apply,
        &registry_fetch(url),
        &crate::central::status,
    )?;
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY").filter(|path| !path.is_empty()) {
        OpenOptions::new().create(true).append(true).open(path)?.write_all(format!("### Release reconciliation: {module} / {destination}\n\n```json\n{json}\n```\n\nFailed versions and source tags remain reserved. Active and uncertain attempts stay pending.\n").as_bytes())?;
    }
    ensure!(
        !apply
            || report
                .iter()
                .all(|entry| matches!(entry.action, "archived" | "finalized")),
        "Some release attempts remain active or need review; inspect the reconciliation report"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{add_failed_attempt, failed_attempt_fixture, failed_response};
    use super::*;
    const ID: &str = "28570f16-da32-4c14-bd2e-c1acc0782365";

    fn central_attempt(
        uploaded: bool,
        receipt: bool,
    ) -> Result<(tempfile::TempDir, std::path::PathBuf, String)> {
        let (temp, root, _) = failed_attempt_fixture()?;
        fs::write(
            root.join("gradle.properties"),
            "GROUP=com.example\nPOM_ARTIFACT_ID=demo\n",
        )?;
        fs::write(
            root.join(".project/setup/config.yaml"),
            "repository: example/demo\ntitle: Demo\ntooling_ref: v1.0.0\nmodules:\n  demo: Demo\n",
        )?;
        fs::create_dir_all(root.join("docs/templates"))?;
        fs::write(
            root.join("docs/templates/MODULE_IMPORT.md.template"),
            "# Install\n{{INSTALLATION}}\n",
        )?;
        git_text(&root, &["add", "."])?;
        git_text(&root, &["commit", "-m", "coordinates"])?;
        git_text(&root, &["push", "origin", "main"])?;
        let source = git_text(&root, &["rev-parse", "HEAD"])?;
        let stable = canonical("demo", "1.0.0", "maven-central");
        let mut record = json!({"module":"demo","version":"1.0.0","source":source,"group":"com.example","artifact":"demo","repository":"maven-central","repository_url":CENTRAL,"phase":"reserved-before-upload","source_tag":stable,"upload_protocol":"guard-v1","verification_schema":1,"verification":crate::release::VerificationConfig::default(),"sha256":SUFFIXES.into_iter().map(|suffix|(suffix,"a".repeat(64))).collect::<BTreeMap<_,_>>()});
        if receipt {
            record["central_deployment_id"] = json!(ID);
        }
        git_text(&root, &["tag", &stable, &source])?;
        git_text(
            &root,
            &[
                "tag",
                "-a",
                "release-pending/demo/1.0.0",
                &source,
                "-m",
                &record.to_string(),
            ],
        )?;
        git_text(&root, &["push", "origin", "--tags"])?;
        if uploaded {
            guard(&root, "demo", "1.0.0", "maven-central")?;
        }
        Ok((temp, root, source))
    }
    fn status(state: &str) -> Value {
        json!({"deploymentId":ID,"deploymentName":"com.example-demo-1.0.0","deploymentState":state})
    }

    #[test]
    fn central_reconciliation_archives_fenced_preupload_and_proven_failure() -> Result<()> {
        for uploaded in [false, true] {
            let (_temp, root, _) = central_attempt(uploaded, uploaded)?;
            let original = remote_oid(&root, "release-pending/demo/1.0.0")?;
            let preview = reconcile_with(
                &root,
                "demo",
                "maven-central",
                &[],
                false,
                &|_, _| Ok(None),
                &|_| Ok(status("FAILED")),
            )?;
            assert_eq!(preview[0].action, "eligible");
            assert_eq!(remote_oid(&root, "release-pending/demo/1.0.0")?, original);
            let report = reconcile_with(
                &root,
                "demo",
                "maven-central",
                &[],
                true,
                &|_, _| Ok(None),
                &|_| Ok(status("FAILED")),
            )?;
            assert_eq!(report[0].action, "archived");
            assert_eq!(remote_oid(&root, "release-failed/demo/1.0.0")?, original);
            assert!(remote_oid(&root, "release-uploading/demo/1.0.0")?.is_none());
            assert!(remote_oid(&root, "demo/v1.0.0-maven-central")?.is_some());
            assert!(reconcile_with(
                &root,
                "demo",
                "maven-central",
                &[],
                true,
                &|_, _| Ok(None),
                &|_| Ok(status("FAILED"))
            )?
            .is_empty());
        }
        Ok(())
    }
    #[test]
    fn central_reconciliation_preserves_active_unknown_partial_and_mismatched_attempts(
    ) -> Result<()> {
        for state in [
            "PENDING",
            "VALIDATING",
            "VALIDATED",
            "PUBLISHING",
            "PUBLISHED",
            "unknown",
        ] {
            let (_temp, root, _) = central_attempt(true, true)?;
            let original = remote_oid(&root, "release-pending/demo/1.0.0")?;
            let report = reconcile_with(
                &root,
                "demo",
                "maven-central",
                &[],
                true,
                &|_, _| Ok(None),
                &|_| Ok(status(state)),
            )?;
            assert_eq!(
                report[0].action,
                if ["PENDING", "VALIDATING", "VALIDATED", "PUBLISHING"].contains(&state) {
                    "active"
                } else {
                    "needs-review"
                }
            );
            assert_eq!(remote_oid(&root, "release-pending/demo/1.0.0")?, original);
        }
        for receipt in [false, true] {
            let (_temp, root, _) = central_attempt(true, receipt)?;
            let report = reconcile_with(
                &root,
                "demo",
                "maven-central",
                &[],
                true,
                &|_, _| Ok(None),
                &|_| anyhow::bail!("HTTP 403"),
            )?;
            assert_eq!(report[0].action, "needs-review");
            assert!(remote_oid(&root, "release-pending/demo/1.0.0")?.is_some());
        }
        let (_temp, root, _) = central_attempt(true, true)?;
        let report = reconcile_with(
            &root,
            "demo",
            "maven-central",
            &[],
            true,
            &|url, _| Ok(url.ends_with(".aar").then(|| b"partial".to_vec())),
            &|_| Ok(status("FAILED")),
        )?;
        assert_eq!(report[0].action, "needs-review");
        assert!(remote_oid(&root, "release-failed/demo/1.0.0")?.is_none());
        let report = reconcile_with(
            &root,
            "demo",
            "maven-central",
            &[],
            true,
            &|_, _| Ok(None),
            &|_| {
                let mut s = status("FAILED");
                s["deploymentId"] = json!("wrong");
                Ok(s)
            },
        )?;
        assert_eq!(report[0].action, "needs-review");
        Ok(())
    }
    #[test]
    fn central_rechecks_status_after_artifact_probes() -> Result<()> {
        let (_temp, root, _) = central_attempt(true, true)?;
        let calls = std::cell::Cell::new(0);
        let report = reconcile_with(
            &root,
            "demo",
            "maven-central",
            &[],
            true,
            &|_, _| Ok(None),
            &|_| {
                let n = calls.get();
                calls.set(n + 1);
                Ok(status(if n == 0 { "FAILED" } else { "PUBLISHING" }))
            },
        )?;
        assert_eq!(report[0].action, "needs-review");
        assert!(remote_oid(&root, "release-pending/demo/1.0.0")?.is_some());
        Ok(())
    }
    #[test]
    fn capture_persists_id_and_rejects_conflicting_receipts() -> Result<()> {
        let (_temp, root, source) = central_attempt(true, false)?;
        let old = remote_oid(&root, "release-pending/demo/1.0.0")?;
        record_central_id(&root, "demo", "1.0.0", ID)?;
        let (record, oid) = read_reservation(&root, "demo", "1.0.0", "maven-central", "")?;
        assert_ne!(old, Some(oid));
        assert_eq!(record["central_deployment_id"], ID);
        assert_eq!(record["source"], source);
        record_central_id(&root, "demo", "1.0.0", ID)?;
        assert!(record_central_id(
            &root,
            "demo",
            "1.0.0",
            "38570f16-da32-4c14-bd2e-c1acc0782365"
        )
        .is_err());
        Ok(())
    }
    #[test]
    fn reconciles_multiple_jitpack_failures_without_corrected_source_requirement() -> Result<()> {
        let (_temp, root, source) = failed_attempt_fixture()?;
        add_failed_attempt(&root, "0.1.0", &source)?;
        add_failed_attempt(&root, "0.1.1", &source)?;
        let report = reconcile_with(
            &root,
            "demo",
            "jitpack",
            &[],
            true,
            &|url, _| failed_response(url, &source),
            &|_| unreachable!(),
        )?;
        assert_eq!(report.len(), 2);
        assert!(report.iter().all(|entry| entry.action == "archived"));
        Ok(())
    }
    #[test]
    fn failed_version_is_burned_even_when_build_inputs_are_unchanged() -> Result<()> {
        let (_temp, root, source) = central_attempt(false, false)?;
        let identity = Identity::new(&root, "demo", "1.0.0", &source)?;
        git_text(
            &root,
            &[
                "tag",
                "-a",
                &identity.tag(),
                &source,
                "-m",
                &serde_json::to_string(&identity)?,
            ],
        )?;
        let (mut record, _) = read_reservation(&root, "demo", "1.0.0", "maven-central", "")?;
        record["release_identity"] = serde_json::to_value(&identity)?;
        git_text(
            &root,
            &[
                "tag",
                "-f",
                "-a",
                "release-pending/demo/1.0.0",
                &source,
                "-m",
                &record.to_string(),
            ],
        )?;
        git_text(&root, &["push", "--force", "origin", "--tags"])?;
        let report = reconcile_with(
            &root,
            "demo",
            "maven-central",
            &[],
            true,
            &|_, _| Ok(None),
            &|_| unreachable!(),
        )?;
        assert_eq!(report[0].action, "archived", "{}", report[0].detail);
        let tags = remote_tags(&root)?;
        assert_eq!(
            select(&root, "demo", &source, "maven-central", &tags, None)?.0,
            "1.0.1"
        );
        assert!(select(
            &root,
            "demo",
            &source,
            "maven-central",
            &tags,
            Some("1.0.0")
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn published_attempt_finalizes_without_upload_or_changing_the_callers_checkout() -> Result<()> {
        use sha2::{Digest, Sha256};
        for newer in [false, true] {
            let (_temp, root, source) = central_attempt(true, false)?;
            let artifacts = crate::confirmation::tests::artifacts("com.example", "demo", "1.0.0");
            let (mut record, _) = read_reservation(&root, "demo", "1.0.0", "maven-central", "")?;
            for (suffix, bytes) in SUFFIXES.iter().zip(&artifacts) {
                record["sha256"][suffix] = json!(format!("{:x}", Sha256::digest(bytes)));
            }
            git_text(
                &root,
                &[
                    "tag",
                    "-f",
                    "-a",
                    "release-pending/demo/1.0.0",
                    &source,
                    "-m",
                    &record.to_string(),
                ],
            )?;
            git_text(
                &root,
                &[
                    "push",
                    "--force",
                    "origin",
                    "refs/tags/release-pending/demo/1.0.0",
                ],
            )?;
            if newer {
                let mut latest = record.clone();
                latest["version"] = json!("1.0.1");
                latest["phase"] = json!("confirmed-public");
                latest["source_tag"] = json!("demo/v1.0.1-maven-central");
                git_text(&root, &["tag", "demo/v1.0.1-maven-central", &source])?;
                fs::create_dir_all(root.join("docs/releases"))?;
                fs::write(
                    root.join("docs/releases/demo.json"),
                    serde_json::to_vec_pretty(&latest)?,
                )?;
                git_text(&root, &["add", "docs/releases/demo.json"])?;
                git_text(&root, &["commit", "-m", "newer confirmed release"])?;
                git_text(&root, &["push", "origin", "main", "--tags"])?;
            }
            let head = git_text(&root, &["rev-parse", "HEAD"])?;
            let fetch = |url: &str, _| {
                if url.ends_with(".asc") {
                    return Ok(Some(b"BEGIN PGP SIGNATURE".to_vec()));
                }
                Ok(SUFFIXES
                    .iter()
                    .position(|s| url.ends_with(s))
                    .map(|i| artifacts[i].clone()))
            };
            let report = reconcile_with(
                &root,
                "demo",
                "maven-central",
                &[],
                true,
                &fetch,
                &|_| unreachable!(),
            )?;
            assert_eq!(report[0].action, "finalized", "{}", report[0].detail);
            assert_eq!(git_text(&root, &["rev-parse", "HEAD"])?, head);
            assert!(remote_oid(&root, "release-pending/demo/1.0.0")?.is_none());
            assert!(remote_oid(&root, "release-uploading/demo/1.0.0")?.is_none());
            fetch_tags(&root)?;
            assert_eq!(
                current(&root, "demo", "maven-central")?.unwrap()["version"],
                if newer { "1.0.1" } else { "1.0.0" }
            );
            assert!(
                remote_file(&root, "docs/releases/history/demo/1.0.0-maven-central.json")?
                    .is_some()
            );
            assert!(reconcile_with(
                &root,
                "demo",
                "maven-central",
                &[],
                true,
                &fetch,
                &|_| unreachable!()
            )?
            .is_empty());
        }
        Ok(())
    }
}
