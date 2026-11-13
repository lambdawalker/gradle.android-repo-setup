mod reconcile;
use crate::confirmation::{
    git_text, read_reservation, registry_fetch, validate_record, validate_record_at,
};
use crate::release::{check_local, coordinates, git, CENTRAL, JITPACK, SUFFIXES};
use crate::release_identity::{self, fingerprint, Identity};
use anyhow::{ensure, Context, Result};
pub(super) use reconcile::{reconcile, track_central_upload};
use regex::Regex;
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

type Fetch<'a> = dyn Fn(&str, bool) -> Result<Option<Vec<u8>>> + 'a;

fn version_key(version: &str) -> Result<[(usize, &str); 3]> {
    let parts: Vec<_> = version.split('.').collect();
    ensure!(
        parts.len() == 3
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part.bytes().all(|byte| byte.is_ascii_digit())
                    && (!part.starts_with('0') || *part == "0")
            }),
        "Expected stable X.Y.Z, got {version:?}"
    );
    Ok([parts[0], parts[1], parts[2]].map(|part| (part.len(), part)))
}

fn compare(left: &str, right: &str) -> Result<Ordering> {
    Ok(version_key(left)?.cmp(&version_key(right)?))
}

fn bump_patch(version: &str) -> Result<String> {
    let (_, patch) = version.rsplit_once('.').context("Missing patch version")?;
    let mut digits = patch.as_bytes().to_vec();
    for digit in digits.iter_mut().rev() {
        if *digit < b'9' {
            *digit += 1;
            return Ok(format!(
                "{}.{}",
                &version[..version.len() - patch.len() - 1],
                String::from_utf8(digits)?
            ));
        }
        *digit = b'0';
    }
    Ok(format!(
        "{}.1{}",
        &version[..version.len() - patch.len() - 1],
        String::from_utf8(digits)?
    ))
}

fn field<'a>(record: &'a Value, name: &str) -> Result<&'a str> {
    record[name]
        .as_str()
        .with_context(|| format!("Missing release field {name}"))
}

fn record_destination(record: &Value) -> Result<&str> {
    match record.get("repository") {
        None => Ok("maven-central"),
        Some(_) => field(record, "repository"),
    }
}

fn scope(destination: &str) -> &'static str {
    if destination == "jitpack" {
        "jitpack/"
    } else {
        ""
    }
}

fn check_source(source: &str) -> Result<()> {
    ensure!(
        Regex::new(r"\A[0-9a-f]{40}\z")?.is_match(source),
        "Invalid source"
    );
    Ok(())
}

fn canonical(module: &str, version: &str, destination: &str) -> String {
    format!("{module}/v{version}-{destination}")
}

fn attempt(kind: &str, module: &str, version: &str, destination: &str) -> String {
    format!("release-{kind}/{}{module}/{version}", scope(destination))
}

fn refname(name: &str) -> String {
    format!("refs/tags/{name}")
}

fn fetch_tags(root: &Path) -> Result<()> {
    git_text(
        root,
        &[
            "fetch",
            "origin",
            "+refs/heads/main:refs/remotes/origin/main",
            "+refs/tags/release-pending/*:refs/tags/release-pending/*",
            "--tags",
        ],
    )?;
    Ok(())
}

fn remote_tags(root: &Path) -> Result<BTreeMap<String, String>> {
    git_text(root, &["ls-remote", "--refs", "--tags", "origin"])?
        .lines()
        .map(|line| {
            let (oid, reference) = line.split_once('\t').context("Malformed remote tag")?;
            let name = reference
                .strip_prefix("refs/tags/")
                .context("Unexpected remote ref")?;
            Ok((name.into(), oid.into()))
        })
        .collect()
}

fn remote_oid(root: &Path, name: &str) -> Result<Option<String>> {
    let reference = refname(name);
    let result = git_text(root, &["ls-remote", "--refs", "origin", &reference])?;
    if result.is_empty() {
        return Ok(None);
    }
    let (oid, found) = result.split_once('\t').context("Malformed remote ref")?;
    ensure!(found == reference, "Unexpected remote ref {found}");
    Ok(Some(oid.to_owned()))
}

fn peel(root: &Path, oid: &str) -> Result<String> {
    git_text(
        root,
        &["rev-parse", "--verify", &format!("{oid}^{{commit}}")],
    )
}

fn ancestor(root: &Path, older: &str, newer: &str) -> Result<()> {
    git_text(root, &["merge-base", "--is-ancestor", older, newer])
        .with_context(|| format!("{older} must be an ancestor of {newer}"))?;
    Ok(())
}

fn remote_file(root: &Path, path: &str) -> Result<Option<Value>> {
    if git_text(root, &["ls-tree", "--name-only", "origin/main", "--", path])?.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&git_text(
        root,
        &["show", &format!("origin/main:{path}")],
    )?)?))
}

fn metadata(module: &str, destination: &str) -> String {
    format!("docs/releases/{}{module}.json", scope(destination))
}

fn current(root: &Path, module: &str, destination: &str) -> Result<Option<Value>> {
    Ok(remote_file(root, &metadata(module, destination))?.filter(|value| !value.is_null()))
}

fn legacy_tags(record: &Value) -> Result<[String; 2]> {
    let module = field(record, "module")?;
    let version = field(record, "version")?;
    let destination = record_destination(record)?;
    Ok([
        format!("{module}/v{version}"),
        format!("{destination}/{module}/v{version}"),
    ])
}

fn record_tag(record: &Value) -> Option<&str> {
    ["source_tag", "legacy_tag"]
        .into_iter()
        .filter_map(|name| record[name].as_str())
        .find(|value| !value.is_empty())
}

fn stable_name(root: &Path, record: &Value) -> Result<String> {
    if let Some(name) = record_tag(record) {
        git_text(root, &["check-ref-format", &refname(name)])?;
        ensure!(!name.starts_with('-'), "Invalid legacy source tag");
        return Ok(name.to_owned());
    }
    let candidates = legacy_tags(record)?;
    for candidate in &candidates {
        if let Some(oid) = remote_oid(root, candidate)? {
            if peel(root, &oid)? == field(record, "source")? {
                return Ok(candidate.clone());
            }
        }
    }
    Ok(candidates[0].clone())
}

fn verify_stable(root: &Path, record: &Value) -> Result<()> {
    release_identity::validate_record(root, record)?;
    let name = stable_name(root, record)?;
    let oid = remote_oid(root, &name)?.context("Missing source tag")?;
    ensure!(
        peel(root, &oid)? == field(record, "source")?,
        "Source tag differs from reservation"
    );
    Ok(())
}

fn output(module: &str, version: &str, source: &str, skip: bool) -> Result<()> {
    let value = format!("module={module}\nversion={version}\nsource={source}\nskip={skip}\n");
    print!("{value}");
    if let Some(path) = std::env::var_os("GITHUB_OUTPUT").filter(|path| !path.is_empty()) {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            .write_all(value.as_bytes())?;
    }
    Ok(())
}

fn history_version<'a>(name: &'a str, module: &str, destination: &str) -> Option<&'a str> {
    let scoped = name
        .strip_prefix(&format!("{module}/v"))
        .and_then(|version| version.strip_suffix(&format!("-{destination}")));
    let legacy = name.strip_prefix(&format!("{destination}/{module}/v"));
    scoped
        .or(legacy)
        .or_else(|| {
            ["pending", "uploading", "failed"]
                .into_iter()
                .find_map(|kind| name.strip_prefix(&attempt(kind, module, "", destination)))
        })
        .filter(|version| version_key(version).is_ok())
}

fn select(
    root: &Path,
    module: &str,
    source: &str,
    destination: &str,
    tags: &BTreeMap<String, String>,
    requested: Option<&str>,
) -> Result<(String, String)> {
    let mut versions = BTreeSet::new();
    for name in tags.keys() {
        for provider in ["maven-central", "jitpack"] {
            if let Some(version) = history_version(name, module, provider) {
                versions.insert(version.to_owned());
            }
        }
        for prefix in [format!("{module}/v"), format!("release-identity/{module}/")] {
            if let Some(version) = name
                .strip_prefix(&prefix)
                .filter(|v| version_key(v).is_ok())
            {
                versions.insert(version.to_owned());
            }
        }
    }
    let listing = git_text(
        root,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            "origin/main",
            "--",
            "docs/releases",
        ],
    )?;
    for path in listing.lines().filter(|p| p.ends_with(".json")) {
        let Some(record) = remote_file(root, path)?.filter(|v| !v.is_null()) else {
            continue;
        };
        if record["module"] == module {
            if let Some(version) = record["version"]
                .as_str()
                .filter(|v| version_key(v).is_ok())
            {
                versions.insert(version.to_owned());
                if let Some(destinations) = record["destinations"].as_object() {
                    crate::docs::validate_archive(root, &record)?;
                    for publication in destinations.values() {
                        verify_stable(root, publication)?;
                    }
                } else {
                    validate_record_at(
                        root,
                        module,
                        version,
                        record_destination(&record)?,
                        &record,
                        false,
                    )?;
                    verify_stable(root, &record)?;
                }
            }
        }
    }
    let latest = versions.iter().max_by_key(|v| version_key(v).ok());
    let selected_fingerprint = fingerprint(root, module, source)?;
    // Validate every shared identity, even when a caller requests a different version.
    let mut identities = BTreeMap::new();
    for version in &versions {
        let identity = release_identity::read(root, module, version)?;
        ensure!(
            identity.is_some() || !tags.contains_key(&release_identity::tag(module, version)),
            "Canonical identity disappeared during selection"
        );
        if let Some(identity) = identity {
            identities.insert(version.clone(), identity);
        }
    }
    if let Some(requested) = requested {
        version_key(requested)?;
        ensure!(
            !tags.contains_key(&attempt("failed", module, requested, destination)),
            "Version was archived as failed at this destination; choose a newer version"
        );
        if let Some(identity) = identities.get(requested) {
            ensure!(
                identity.fingerprint == selected_fingerprint,
                "Explicit canonical version has different build inputs; choose a newer version"
            );
            ancestor(root, &identity.source, source)?;
            return Ok((requested.into(), identity.source.clone()));
        }
        ensure!(
            latest.is_none_or(|latest| compare(requested, latest).is_ok_and(Ordering::is_gt)),
            "Explicit version must be newer than all module history; use Finalize for recovery"
        );
        return Ok((requested.into(), source.into()));
    }
    let Some(latest) = latest else {
        return Ok(("0.1.0".into(), source.into()));
    };
    if let Some(identity) = identities.get(latest) {
        ancestor(root, &identity.source, source)?;
        if identity.fingerprint == selected_fingerprint
            && !tags.contains_key(&attempt("failed", module, latest, destination))
        {
            return Ok((latest.clone(), identity.source.clone()));
        }
    }
    Ok((bump_patch(latest)?, source.into()))
}

fn ensure_available(
    root: &Path,
    module: &str,
    destination: &str,
    tags: &BTreeMap<String, String>,
    pending_override: Option<(&str, &str)>,
) -> Result<()> {
    if destination == "maven-central" {
        ensure!(!tags.keys().any(|name| {
            ["release-pending/", "release-uploading/"].iter().any(|prefix| {
                name.strip_prefix(prefix).is_some_and(|version| version_key(version).is_ok())
            })
        }), "Unresolved legacy paired release; finalize it using the old workflow before independent releases");
    }
    let pending = attempt("pending", module, "", destination);
    let uploading = attempt("uploading", module, "", destination);
    let mut unresolved = BTreeSet::new();
    for name in tags
        .keys()
        .filter(|name| name.starts_with(&pending) || name.starts_with(&uploading))
    {
        if let (Some((version, source)), Some(old_version)) =
            (pending_override, name.strip_prefix(&pending))
        {
            ensure!(
                compare(version, old_version)?.is_gt(),
                "JitPack override requires a version newer than every pending attempt"
            );
            let (record, _) =
                read_reservation(root, module, old_version, destination, scope(destination))?;
            verify_stable(root, &record)?;
            ancestor(root, field(&record, "source")?, source)?;
            println!("JitPack override: preserving {name} and proceeding with {version}");
        } else {
            unresolved.insert(name);
        }
    }
    ensure!(
        unresolved.is_empty(),
        "Unresolved {module} attempt {unresolved:?}; run finalization, never re-upload"
    );
    Ok(())
}

fn central_versions(group: &str, artifact: &str, fetch: &Fetch<'_>) -> Result<Vec<String>> {
    let url = format!(
        "{CENTRAL}/{group_path}/{artifact}/maven-metadata.xml",
        group_path = group.replace('.', "/")
    );
    let Some(bytes) = fetch(&url, true)? else {
        return Ok(Vec::new());
    };
    let xml = std::str::from_utf8(&bytes)?;
    let document = roxmltree::Document::parse(xml)?;
    let root = document.root_element();
    let child = |name| root.children().find(|node| node.has_tag_name(name));
    ensure!(
        root.has_tag_name("metadata")
            && child("groupId").and_then(|node| node.text()) == Some(group)
            && child("artifactId").and_then(|node| node.text()) == Some(artifact),
        "Invalid Central metadata identity"
    );
    let versions = child("versioning")
        .and_then(|node| node.children().find(|node| node.has_tag_name("versions")))
        .context("Malformed/empty Central metadata")?;
    let found = versions
        .children()
        .filter(|node| node.has_tag_name("version"))
        .map(|node| {
            node.text()
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
                .context("Malformed/empty Central metadata")
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(!found.is_empty(), "Malformed/empty Central metadata");
    Ok(found)
}

fn artifact_base(url: &str, group: &str, artifact: &str, version: &str) -> String {
    format!(
        "{url}/{}/{artifact}/{version}/{artifact}-{version}",
        group.replace('.', "/")
    )
}

fn artifacts_exist(base: &str, fetch: &Fetch<'_>) -> Result<bool> {
    for suffix in SUFFIXES.into_iter().chain([".jar"]) {
        if fetch(&format!("{base}{suffix}"), true)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn archive_attempt(root: &Path, archive: &str, attempts: &[(&str, &str)]) -> Result<()> {
    let archive_ref = refname(archive);
    ensure!(
        remote_oid(root, archive)?.is_none(),
        "Failure archive already exists; reconcile history"
    );
    let mut args = vec![
        "push".to_owned(),
        "--atomic".to_owned(),
        format!("--force-with-lease={archive_ref}:"),
    ];
    for (name, oid) in attempts {
        args.push(format!("--force-with-lease={}:{oid}", refname(name)));
    }
    args.push("origin".to_owned());
    args.push(format!("{}:{archive_ref}", attempts[0].1));
    for (name, _) in attempts {
        args.push(format!(":{}", refname(name)));
    }
    git_text(root, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
    for (name, oid) in attempts {
        git_text(root, &["update-ref", "-d", &refname(name), oid])?;
    }
    Ok(())
}

fn verify_failed_jitpack(record: &Value, fetch: &Fetch<'_>) -> Result<()> {
    let group = field(record, "group")?;
    let artifact = field(record, "artifact")?;
    let consumer = field(record, "consumer_version")?;
    let check_failure = || -> Result<()> {
        let bytes = fetch(
            &format!("{JITPACK}/api/builds/{group}/{artifact}/{consumer}"),
            false,
        )?
        .context("Missing JitPack build status")?;
        let status: Value = serde_json::from_slice(&bytes)?;
        ensure!(
            matches!(
                status["status"]
                    .as_str()
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("error" | "failed")
            ),
            "JitPack status {:?} is not a proven terminal failure; preserve pending journal",
            status["status"].as_str().unwrap_or("missing")
        );
        ensure!(
            status["commit"] == field(record, "source")?,
            "JitPack failure source does not match the reserved source"
        );
        for (name, expected) in [
            ("group", group),
            ("artifact", artifact),
            ("version", consumer),
        ] {
            ensure!(
                status.get(name).is_none_or(|value| value == expected),
                "JitPack failure identity differs from the reservation"
            );
        }
        Ok(())
    };
    check_failure()?;
    ensure!(
        !artifacts_exist(&artifact_base(JITPACK, group, artifact, consumer), fetch)?,
        "JitPack attempt has public artifacts; preserve pending journal"
    );
    check_failure()?;
    Ok(())
}

#[cfg(test)]
fn recover_failed_jitpack(
    root: &Path,
    module: &str,
    tags: &BTreeMap<String, String>,
    version: &str,
    source: &str,
    fetch: &Fetch<'_>,
) -> Result<()> {
    let prefix = attempt("pending", module, "", "jitpack");
    let mut attempts = tags.keys().filter(|name| name.starts_with(&prefix));
    let Some(name) = attempts.next() else {
        return Ok(());
    };
    let uploading = attempt("uploading", module, "", "jitpack");
    ensure!(
        attempts.next().is_none() && !tags.keys().any(|name| name.starts_with(&uploading)),
        "Multiple or upload-started JitPack attempts require manual reconciliation"
    );
    let old_version = &name[prefix.len()..];
    let (record, oid) = read_reservation(root, module, old_version, "jitpack", "jitpack/")?;
    let old_source = field(&record, "source")?;
    ensure!(
        compare(version, old_version)?.is_gt() && source != old_source,
        "Failed JitPack recovery requires a newer version and corrected source"
    );
    ancestor(root, old_source, source)?;
    verify_stable(root, &record)?;
    // Use the same exact-version history, phase, provenance and lease checks as
    // explicit reconciliation. A newer confirmed release is not this attempt.
    // Keep the object inspected above through reconciliation and the atomic
    // archive lease; refreshing it would invalidate the ancestry checks.
    let inspected = BTreeMap::from([(name.clone(), oid)]);
    let report = reconcile_failed_jitpack_snapshot(
        root,
        module,
        &[old_version.into()],
        true,
        fetch,
        &inspected,
    )?;
    ensure!(
        report.len() == 1 && report[0].action == "archived",
        "Cannot archive {name}; the attempt is preserved: {}. Investigate or finalize it first. \
         For an intentional separate release, supply --version X.Y.Z newer than all module history \
         and --allow-pending-jitpack (Actions: set version and bypass_version_verification=true); \
         this retains the unresolved attempt",
        report
            .first()
            .map(|entry| entry.detail.as_str())
            .unwrap_or("pending journal changed; retry the review")
    );
    println!("Archived failed JitPack {module} {old_version}; source tag retained; advancing to {version}");
    Ok(())
}

#[derive(serde::Serialize)]
pub(super) struct ReconciledAttempt {
    version: String,
    action: &'static str,
    detail: String,
}

fn ensure_unconfirmed_jitpack(root: &Path, module: &str, version: &str) -> Result<()> {
    if let Some(record) = current(root, module, "jitpack")? {
        version_key(field(&record, "version")?)?;
        ensure!(
            record["version"] != version,
            "Version has a confirmed release record"
        );
    }
    for filename in [format!("{version}-jitpack.json"), format!("{version}.json")] {
        let path = format!("{}/{module}/{filename}", crate::docs::HISTORY);
        if let Some(entry) = remote_file(root, &path)? {
            ensure!(
                entry["destinations"]
                    .as_object()
                    .context("Malformed release history")?
                    .get("jitpack")
                    .is_none(),
                "Version has confirmed JitPack release history"
            );
        }
    }
    ensure!(
        remote_oid(root, &attempt("uploading", module, version, "jitpack"))?.is_none(),
        "Upload-started attempt requires manual review"
    );
    Ok(())
}

fn reconcile_failed_jitpack_with(
    root: &Path,
    module: &str,
    versions: &[String],
    apply: bool,
    fetch: &Fetch<'_>,
) -> Result<Vec<ReconciledAttempt>> {
    let tags = remote_tags(root)?;
    reconcile_failed_jitpack_snapshot(root, module, versions, apply, fetch, &tags)
}

fn reconcile_failed_jitpack_snapshot(
    root: &Path,
    module: &str,
    versions: &[String],
    apply: bool,
    fetch: &Fetch<'_>,
    tags: &BTreeMap<String, String>,
) -> Result<Vec<ReconciledAttempt>> {
    coordinates(root, module, "0.1.0", "jitpack", true)?;
    for version in versions {
        version_key(version)?;
    }
    let prefix = attempt("pending", module, "", "jitpack");
    let mut report = Vec::new();
    for (name, expected_oid) in tags.iter().filter(|(name, _)| name.starts_with(&prefix)) {
        let version = &name[prefix.len()..];
        if !versions.is_empty() && !versions.iter().any(|selected| selected == version) {
            continue;
        }
        let result = (|| -> Result<String> {
            version_key(version)?;
            let (record, oid) = read_reservation(root, module, version, "jitpack", "jitpack/")?;
            ensure!(
                oid == *expected_oid,
                "Pending journal changed during reconciliation"
            );
            ensure!(
                record["phase"] == "reserved-before-upload",
                "Journal is already confirmed"
            );
            // Valid legacy journals use module/vX.Y.Z (and their original
            // consumer version). Verify that tag instead of rewriting identity.
            verify_stable(root, &record)?;
            ensure_unconfirmed_jitpack(root, module, version)?;
            verify_failed_jitpack(&record, fetch)?;
            // Recheck Git state after network requests. Atomic pending deletion below
            // races safely with finalization and upload guards through the same lease.
            verify_stable(root, &record)?;
            ensure_unconfirmed_jitpack(root, module, version)?;
            let archive = attempt("failed", module, version, "jitpack");
            ensure!(
                remote_oid(root, &archive)?.is_none(),
                "Failure archive already exists"
            );
            if apply {
                archive_attempt(root, &archive, &[(name, &oid)])?;
            }
            Ok(format!(
                "{} -> {archive} (object {oid}, source {})",
                name,
                field(&record, "source")?
            ))
        })();
        report.push(match result {
            Ok(detail) => ReconciledAttempt {
                version: version.into(),
                action: if apply { "archived" } else { "eligible" },
                detail,
            },
            Err(error) => ReconciledAttempt {
                version: version.into(),
                action: "needs-review",
                detail: error.to_string(),
            },
        });
    }
    Ok(report)
}

pub(super) fn reconcile_failed_jitpack(
    root: &Path,
    module: &str,
    versions: &[String],
    apply: bool,
) -> Result<()> {
    fetch_tags(root)?;
    let report =
        reconcile_failed_jitpack_with(root, module, versions, apply, &registry_fetch(JITPACK))?;
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY").filter(|path| !path.is_empty()) {
        let mode = if apply { "Apply" } else { "Preview" };
        let summary = format!("### {mode} failed JitPack attempts\n\n```json\n{json}\n```\n\nCanonical source tags and confirmed releases are retained. Entries marked needs-review require investigation.\n");
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            .write_all(summary.as_bytes())?;
    }
    ensure!(
        !apply || report.iter().all(|entry| entry.action != "needs-review"),
        "Some attempts were preserved and need review; inspect the reconciliation report"
    );
    Ok(())
}

pub(super) fn prepare(
    root: &Path,
    module: &str,
    requested: Option<&str>,
    destination: &str,
    allow_pending_jitpack: bool,
    skip_central_version_check: bool,
) -> Result<()> {
    let requested = requested.map(str::trim).filter(|value| !value.is_empty());
    ensure!(
        !allow_pending_jitpack || destination == "jitpack",
        "JitPack override requires the JitPack destination"
    );
    ensure!(
        !allow_pending_jitpack || requested.is_some(),
        "JitPack override requires an explicit version (X.Y.Z)"
    );
    ensure!(
        !skip_central_version_check || destination == "maven-central",
        "Maven Central version override requires the Maven Central destination"
    );
    ensure!(
        !skip_central_version_check || requested.is_some(),
        "Maven Central version override requires an explicit version (X.Y.Z)"
    );
    let coords = coordinates(
        root,
        module,
        requested.unwrap_or("0.1.0"),
        destination,
        true,
    )?;
    if !allow_pending_jitpack {
        reconcile(root, module, destination, &[], true)?;
    }
    fetch_tags(root)?;
    let tags = remote_tags(root)?;
    if destination == "maven-central" {
        ensure_available(root, module, destination, &tags, None)?;
    }
    let head = git_text(root, &["rev-parse", "HEAD"])?;
    ancestor(root, &head, "origin/main")?;
    let (version, source) = select(root, module, &head, destination, &tags, requested)?;
    if destination == "jitpack" {
        if allow_pending_jitpack {
            ensure!(
                source == head,
                "JitPack override requires a new version from the selected workflow source"
            );
            ensure_available(root, module, destination, &tags, Some((&version, &source)))?;
        } else {
            ensure_available(root, module, destination, &remote_tags(root)?, None)?;
        }
    }
    if let Some(record) = current(root, module, destination)? {
        if record["version"] == version {
            validate_record(root, module, &version, destination, &record)?;
            verify_stable(root, &record)?;
            ensure!(
                record["source"] == source,
                "Destination version has conflicting source"
            );
            return output(module, &version, &source, true);
        }
    }
    if destination == "maven-central" && !skip_central_version_check {
        let published =
            central_versions(&coords.group, &coords.artifact, &registry_fetch(CENTRAL))?;
        ensure!(!published.contains(&version),
            "Version exists at destination without a matching confirmed record; investigate before upload");
        ensure!(
            !published
                .iter()
                .any(|published| compare(published, &version).is_ok_and(Ordering::is_gt)),
            "Destination has newer untracked releases; reconcile provenance"
        );
    }
    output(module, &version, &source, false)
}

pub(super) fn reserve(
    root: &Path,
    module: &str,
    version: &str,
    source: &str,
    destination: &str,
    run_id: &str,
    allow_pending_jitpack: bool,
) -> Result<()> {
    ensure!(
        !allow_pending_jitpack || destination == "jitpack",
        "JitPack override requires the JitPack destination"
    );
    let record = serde_json::to_value(check_local(
        root,
        module,
        version,
        source,
        destination,
        run_id,
    )?)?;
    reserve_validated(
        root,
        &record,
        allow_pending_jitpack,
        &registry_fetch(CENTRAL),
    )
}

// Distinct objects keep Git's up-to-date optimization from skipping a
// create-only lease even when runners share a run ID and tagger timestamp.
fn tag_nonce() -> Result<String> {
    let mut random = [0; 16];
    getrandom::fill(&mut random)
        .map_err(|_| anyhow::anyhow!("Could not generate a Git tag nonce"))?;
    Ok(random
        .into_iter()
        .flat_map(|byte| (0..8).map(move |bit| if byte & (1 << bit) == 0 { ' ' } else { '\t' }))
        .collect())
}

fn reserve_validated(
    root: &Path,
    record: &Value,
    allow_pending_jitpack: bool,
    fetch_central: &Fetch<'_>,
) -> Result<()> {
    let module = field(record, "module")?;
    let version = field(record, "version")?;
    let source = field(record, "source")?;
    let destination = record_destination(record)?;
    if destination == "maven-central" {
        let base = artifact_base(
            CENTRAL,
            field(record, "group")?,
            field(record, "artifact")?,
            version,
        );
        ensure!(
            !artifacts_exist(&base, fetch_central)?,
            "Immutable version already has public artifacts"
        );
    }
    fetch_tags(root)?;
    let tags = remote_tags(root)?;
    let (selected_version, selected_source) =
        select(root, module, source, destination, &tags, Some(version))?;
    ensure!(
        selected_version == version && selected_source == source,
        "Reservation must use canonical source selected by Prepare"
    );
    let identity = Identity::new(root, module, version, source)?;
    ensure!(
        record["release_identity"] == serde_json::to_value(&identity)?,
        "Reservation identity differs from build inputs"
    );
    let existing_identity = release_identity::read(root, module, version)?;
    let pending_override = allow_pending_jitpack.then_some((version, source));
    ensure_available(
        root,
        module,
        destination,
        &remote_tags(root)?,
        pending_override,
    )?;
    ancestor(root, source, "origin/main")?;
    let stable = canonical(module, version, destination);
    let stable_ref = refname(&stable);
    let pending = attempt("pending", module, version, destination);
    let pending_ref = refname(&pending);
    let pending_lease = format!("--force-with-lease={pending_ref}:");
    let stable_lease = format!("--force-with-lease={stable_ref}:");
    let identity_ref = refname(&identity.tag());
    let identity_lease = format!("--force-with-lease={identity_ref}:");
    let mut args = vec!["push", "--atomic", &pending_lease];
    if existing_identity.is_none() {
        git_text(
            root,
            &[
                "-c",
                "tag.gpgSign=false",
                "tag",
                "-a",
                "--cleanup=verbatim",
                &identity.tag(),
                source,
                "-m",
                &format!("{}{}\n", serde_json::to_string(&identity)?, tag_nonce()?),
            ],
        )?;
        args.push(&identity_lease);
    }

    let stable_oid = remote_oid(root, &stable)?;
    if let Some(oid) = &stable_oid {
        ensure!(
            existing_identity.is_some(),
            "Legacy destination version cannot be adopted into canonical identity"
        );
        ensure!(
            peel(root, oid)? == source,
            "Canonical tag has different source"
        );
    } else {
        args.push(&stable_lease);
    }
    args.push("origin");
    if existing_identity.is_none() {
        args.push(&identity_ref);
    }
    if stable_oid.is_none() {
        git_text(
            root,
            &[
                "-c",
                "tag.gpgSign=false",
                "tag",
                "-a",
                &stable,
                source,
                "-m",
                &format!("Module release {module} {version}; publication tracked separately"),
            ],
        )?;
        args.push(&stable_ref);
    }
    git_text(
        root,
        &[
            "-c",
            "tag.gpgSign=false",
            "tag",
            "-a",
            "--cleanup=verbatim",
            &pending,
            source,
            "-m",
            &format!("{}{}\n", serde_json::to_string(record)?, tag_nonce()?),
        ],
    )?;
    args.push(&pending_ref);
    git_text(root, &args)?;
    Ok(())
}

pub(super) fn guard(root: &Path, module: &str, version: &str, destination: &str) -> Result<()> {
    let coords = coordinates(root, module, version, destination, true)?;
    let (record, pending_oid) =
        read_reservation(root, module, version, destination, scope(destination))?;
    let source = field(&record, "source")?;
    ensure!(
        source == git_text(root, &["rev-parse", "HEAD"])?
            && destination == "maven-central"
            && record["group"] == coords.group
            && record["artifact"] == coords.artifact,
        "Reservation differs from source/module"
    );
    if let Some(record) = current(root, module, destination)? {
        ensure!(
            record["version"] != version,
            "Version already finalized at destination"
        );
    }
    let marker = attempt("uploading", module, version, destination);
    ensure!(
        remote_oid(root, &marker)?.is_none(),
        "Upload already started; finalize without re-uploading"
    );
    let pending = attempt("pending", module, version, destination);
    publish_guard(root, &pending, &pending_oid, &marker, source)
}

fn publish_guard(
    root: &Path,
    pending: &str,
    pending_oid: &str,
    marker: &str,
    source: &str,
) -> Result<()> {
    let mut journal = git(root, &["cat-file", "tag", pending_oid])?;
    ensure!(
        journal.status.success(),
        "Could not read the pinned reservation"
    );
    // A real update makes Git enforce the pending lease. A no-op push silently
    // skips the lease. Trailing JSON whitespace preserves the journal payload.
    journal.stdout.extend_from_slice(tag_nonce()?.as_bytes());
    journal.stdout.push(b'\n');
    let mut mktag = Command::new("git")
        .arg("mktag")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    mktag
        .stdin
        .take()
        .context("Missing Git input")?
        .write_all(&journal.stdout)?;
    let tag = mktag.wait_with_output()?;
    ensure!(
        tag.status.success(),
        "Could not renew reservation: {}",
        String::from_utf8_lossy(&tag.stderr).trim()
    );
    let renewed = std::str::from_utf8(&tag.stdout)?.trim();
    let marker_ref = refname(marker);
    let pending_ref = refname(pending);
    git_text(
        root,
        &[
            "-c",
            "tag.gpgSign=false",
            "tag",
            "-a",
            "--cleanup=verbatim",
            marker,
            source,
            "-m",
            &format!("Upload may have started: {pending}{}\n", tag_nonce()?),
        ],
    )?;
    let marker_oid = git_text(root, &["rev-parse", "--verify", &marker_ref])?;
    let pushed = git_text(
        root,
        &[
            "push",
            "--atomic",
            &format!("--force-with-lease={pending_ref}:{pending_oid}"),
            &format!("--force-with-lease={marker_ref}:"),
            "origin",
            &format!("{renewed}:{pending_ref}"),
            &format!("{marker_oid}:{marker_ref}"),
        ],
    );
    if let Err(error) = pushed {
        git_text(root, &["update-ref", "-d", &marker_ref, &marker_oid])?;
        return Err(error);
    }
    git_text(root, &["update-ref", &pending_ref, renewed, pending_oid])?;
    Ok(())
}

pub(super) fn prepare_finalization(
    root: &Path,
    module: &str,
    version: &str,
    source: Option<&str>,
    destination: &str,
) -> Result<()> {
    coordinates(root, module, version, destination, true)?;
    let source = source.filter(|value| !value.is_empty());
    if let Some(source) = source {
        check_source(source)?;
    }
    fetch_tags(root)?;
    let current = current(root, module, destination)?;
    if remote_oid(root, &attempt("pending", module, version, destination))?.is_some() {
        let (record, _) = read_reservation(root, module, version, destination, scope(destination))?;
        if record.get("source_tag").is_some() {
            verify_stable(root, &record).context("Pending source tag differs from reservation")?;
        }
        let reserved = field(&record, "source")?;
        ensure!(
            source.is_none_or(|source| source == reserved),
            "Conflicting source"
        );
        ancestor(root, reserved, "origin/main")?;
        if let Some(current) = current {
            ensure!(
                compare(field(&current, "version")?, version)?.is_lt(),
                "Pending release would overwrite newer documentation"
            );
        }
        return output(module, version, reserved, false);
    }
    ensure!(
        remote_oid(root, &attempt("uploading", module, version, destination))?.is_none(),
        "No matching pending or completed release"
    );
    let current = current.context("No matching pending or completed release")?;
    let current_version = field(&current, "version")?;
    validate_record(root, module, current_version, destination, &current)?;
    ensure!(
        current["phase"] == "confirmed-public" && !compare(current_version, version)?.is_lt(),
        "Stable tag lacks confirmed docs"
    );
    let archived;
    let record = if current_version == version {
        &current
    } else {
        let mut found = None;
        for filename in [
            format!("{version}-{destination}.json"),
            format!("{version}.json"),
        ] {
            let path = format!("{}/{module}/{filename}", crate::docs::HISTORY);
            if let Some(entry) = remote_file(root, &path)? {
                crate::docs::validate_archive(root, &entry)?;
                if let Some(record) = entry["destinations"].get(destination) {
                    found = Some(record.clone());
                    break;
                }
            }
        }
        archived = found.context("No confirmed record for this destination and version")?;
        validate_record(root, module, version, destination, &archived)?;
        &archived
    };
    verify_stable(root, record).context("Confirmed source differs from stable tag")?;
    let confirmed = field(record, "source")?;
    let latest = field(&current, "source")?;
    ensure!(
        source.is_none_or(|source| source == confirmed),
        "Conflicting source"
    );
    ancestor(root, confirmed, latest)?;
    ancestor(root, latest, "origin/main")?;
    output(module, version, confirmed, true)
}

pub(super) fn finalize(
    root: &Path,
    module: &str,
    version: &str,
    source: &str,
    destination: &str,
) -> Result<()> {
    coordinates(root, module, version, destination, true)?;
    check_source(source)?;
    let record: Value = serde_json::from_slice(&fs::read(root.join(format!(
        "build/{}confirmed-{module}.json",
        scope(destination)
    )))?)?;
    validate_record(root, module, version, destination, &record)?;
    ensure!(
        record["source"] == source && record["phase"] == "confirmed-public",
        "Confirmation mismatch"
    );

    fetch_tags(root)?;
    let (journal, pending_oid) =
        read_reservation(root, module, version, destination, scope(destination))?;
    ensure!(
        crate::confirmation::publication_suffixes(root, &journal)?
            == crate::confirmation::publication_suffixes(root, &record)?,
        "Confirmation changed primary artifact packaging"
    );
    let mut expected = journal.clone();
    expected["phase"] = json!("confirmed-public");
    if destination == "jitpack" {
        expected["sha256"] = record["sha256"].clone();
    }
    ensure!(expected == record, "Confirmation differs from reservation");
    ancestor(root, source, "origin/main")?;
    git_text(
        root,
        &[
            "diff",
            "--exit-code",
            "HEAD",
            "origin/main",
            "--",
            "scripts",
            ":(exclude)scripts/tests",
            "publishing",
            ".project/setup",
            "docs/templates/MODULE_IMPORT.md.template",
        ],
    )?;
    let update_latest = if let Some(current) = current(root, module, destination)? {
        match compare(field(&current, "version")?, version)? {
            Ordering::Less => true,
            Ordering::Equal => {
                ensure!(
                    current == record,
                    "Existing confirmation differs from the reserved publication"
                );
                false
            }
            Ordering::Greater => false,
        }
    } else {
        true
    };
    let stable = stable_name(root, &journal)?;
    let stable_oid = remote_oid(root, &stable)?;
    if let Some(oid) = &stable_oid {
        ensure!(peel(root, oid)? == source, "Stable tag source mismatch");
    }
    let uploading = attempt("uploading", module, version, destination);
    let uploading_oid = remote_oid(root, &uploading)?;
    if let Some(oid) = &uploading_oid {
        ensure!(peel(root, oid)? == source, "Upload marker source mismatch");
    }

    git_text(root, &["checkout", "--detach", "origin/main"])?;
    crate::docs::seed(root)?;
    crate::docs::archive_record(root, &record)?;
    let metadata = metadata(module, destination);
    let path = root.join(&metadata);
    fs::create_dir_all(
        path.parent()
            .context("Missing release metadata directory")?,
    )?;
    if update_latest {
        fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&record)?),
        )?;
    }
    crate::docs::documentation(root, false)?;
    git_text(root, &["add", &metadata, "IMPORT.md", crate::docs::HISTORY])?;
    if !git(root, &["diff", "--cached", "--quiet"])?
        .status
        .success()
    {
        git_text(
            root,
            &[
                "-c",
                "user.name=github-actions[bot]",
                "-c",
                "user.email=41898282+github-actions[bot]@users.noreply.github.com",
                "-c",
                "commit.gpgSign=false",
                "commit",
                "-m",
                &format!("docs: confirm {module} {version}"),
            ],
        )?;
    }

    let pending = refname(&attempt("pending", module, version, destination));
    let stable_ref = refname(&stable);
    let marker_ref = refname(&uploading);
    let pending_lease = format!("--force-with-lease={pending}:{pending_oid}");
    let stable_lease = format!("--force-with-lease={stable_ref}:");
    let marker_lease = uploading_oid
        .as_ref()
        .map(|oid| format!("--force-with-lease={marker_ref}:{oid}"));
    let delete_pending = format!(":{pending}");
    let delete_marker = format!(":{marker_ref}");
    let mut args = vec!["push", "--atomic", &pending_lease];
    if stable_oid.is_none() {
        git_text(
            root,
            &[
                "-c",
                "user.name=github-actions[bot]",
                "-c",
                "user.email=41898282+github-actions[bot]@users.noreply.github.com",
                "-c",
                "tag.gpgSign=false",
                "tag",
                "-a",
                &stable,
                source,
                "-m",
                &format!("Module release {module} {version}"),
            ],
        )?;
        args.push(&stable_lease);
    }
    if let Some(lease) = &marker_lease {
        args.push(lease);
    }
    args.extend(["origin", "HEAD:refs/heads/main", &delete_pending]);
    if stable_oid.is_none() {
        args.push(&stable_ref);
    }
    if marker_lease.is_some() {
        args.push(&delete_marker);
    }
    git_text(root, &args)?;
    Ok(())
}

pub(super) fn abort_unconfirmed(
    root: &Path,
    module: &str,
    version: &str,
    destination: &str,
) -> Result<()> {
    ensure!(
        destination == "maven-central",
        "abort-unconfirmed supports Maven Central only"
    );
    reconcile(root, module, destination, &[version.to_owned()], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_jvm_jar_publication_blocks_reallocation() -> Result<()> {
        assert!(artifacts_exist(
            "https://example.invalid/artifact",
            &|url, _| {
                Ok(url
                    .ends_with("artifact.jar")
                    .then(|| b"already published".to_vec()))
            }
        )?);
        Ok(())
    }

    #[test]
    fn archived_coordinates_remain_valid_after_publication_coordinates_change() -> Result<()> {
        let (_temp, root, _) = failed_attempt_fixture()?;
        fs::write(
            root.join(".project/setup/config.yaml"),
            "repository: example/demo\ntitle: Demo\ntooling_ref: v1.0.0\nmodules:\n  demo: Demo\n",
        )?;
        fs::write(
            root.join("gradle.properties"),
            "GROUP=com.old\nPOM_ARTIFACT_ID=old-demo\n",
        )?;
        git_text(&root, &["add", "."])?;
        git_text(&root, &["commit", "-m", "old coordinates"])?;
        let old_source = git_text(&root, &["rev-parse", "HEAD"])?;
        let record = |version: &str, source: &str, group: &str, artifact: &str| {
            json!({
                "module":"demo", "version":version, "source":source, "group":group, "artifact":artifact,
                "repository":"maven-central", "repository_url":CENTRAL, "phase":"confirmed-public",
                "source_tag":format!("demo/v{version}-maven-central"),
                "sha256": SUFFIXES.into_iter().map(|suffix| (suffix, "a".repeat(64))).collect::<BTreeMap<_,_>>()
            })
        };
        let old_record = record("1.0.0", &old_source, "com.old", "old-demo");
        git_text(&root, &["tag", "demo/v1.0.0-maven-central", &old_source])?;
        fs::write(
            root.join("gradle.properties"),
            "GROUP=com.new\nPOM_ARTIFACT_ID=new-demo\n",
        )?;
        git_text(&root, &["add", "."])?;
        git_text(&root, &["commit", "-m", "rename publication coordinates"])?;
        let latest_source = git_text(&root, &["rev-parse", "HEAD"])?;
        let latest_record = record("1.0.1", &latest_source, "com.new", "new-demo");
        git_text(&root, &["tag", "demo/v1.0.1-maven-central", &latest_source])?;
        let archive = json!({"schema":1, "module":"demo", "version":"1.0.0", "source":old_source,
            "documentation_ref":old_source, "destinations":{"maven-central":old_record}});
        let archive_path = root.join("docs/releases/history/demo/1.0.0-maven-central.json");
        fs::create_dir_all(archive_path.parent().unwrap())?;
        let original_archive = serde_json::to_vec_pretty(&archive)?;
        fs::write(&archive_path, &original_archive)?;
        fs::write(
            root.join("docs/releases/demo.json"),
            serde_json::to_vec(&latest_record)?,
        )?;
        git_text(&root, &["add", "."])?;
        git_text(&root, &["commit", "-m", "record releases"])?;
        git_text(&root, &["push", "origin", "main", "--tags"])?;
        let head = git_text(&root, &["rev-parse", "HEAD"])?;
        let tags = remote_tags(&root)?;
        assert_eq!(
            select(&root, "demo", &head, "maven-central", &tags, None)?,
            ("1.0.2".into(), head)
        );
        assert_eq!(fs::read(&archive_path)?, original_archive);
        Ok(())
    }

    #[test]
    fn competing_providers_cannot_reserve_different_sources_for_one_version() -> Result<()> {
        let (temp, seed, _) = failed_attempt_fixture()?;
        fs::write(seed.join("gradle.properties"), "GROUP=com.example\n")?;
        git_text(&seed, &["add", "."])?;
        git_text(&seed, &["commit", "-m", "build properties"])?;
        let first = git_text(&seed, &["rev-parse", "HEAD"])?;
        fs::write(seed.join("unrelated"), "no build change")?;
        git_text(&seed, &["add", "."])?;
        git_text(&seed, &["commit", "-m", "unrelated"])?;
        let second = git_text(&seed, &["rev-parse", "HEAD"])?;
        git_text(&seed, &["push", "origin", "main"])?;
        let remote = temp.path().join("origin.git");
        let mut candidates = Vec::new();
        for (name, destination, source) in [
            ("left", "maven-central", &first),
            ("right", "jitpack", &second),
        ] {
            git_text(
                temp.path(),
                &["clone", "--branch", "main", remote.to_str().unwrap(), name],
            )?;
            let root = temp.path().join(name);
            configure_fixture_git(&root)?;
            let record = json!({"module":"demo", "version":"1.0.0", "source":source, "repository":destination, "group":"com.example", "artifact":"demo", "release_identity":Identity::new(&root,"demo","1.0.0",source)?});
            candidates.push((root, record));
        }
        let ready = std::sync::Barrier::new(2);
        let results = std::thread::scope(|threads| {
            let handles: Vec<_> = candidates
                .iter()
                .map(|(root, record)| {
                    let ready = &ready;
                    threads.spawn(move || {
                        ready.wait();
                        reserve_validated(root, record, false, &|_, _| Ok(None))
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        fetch_tags(&seed)?;
        let identity = release_identity::read(&seed, "demo", "1.0.0")?.unwrap();
        let tags = remote_tags(&seed)?;
        assert_eq!(tags.len(), 3);
        assert!(identity.source == first || identity.source == second);
        // A forced provider override still cannot use an existing version with another source.
        let loser = candidates
            .iter()
            .find(|(_, r)| r["source"] != identity.source)
            .unwrap();
        assert!(reserve_validated(&loser.0, &loser.1, true, &|_, _| Ok(None)).is_err());
        let other = if identity.source == first {
            "jitpack"
        } else {
            "maven-central"
        };
        let canonical_oid = remote_oid(&seed, &identity.tag())?;
        let reuse = json!({"module":"demo", "version":"1.0.0", "source":identity.source,
            "repository":other, "group":"com.example", "artifact":"demo", "release_identity":identity});
        reserve_validated(&seed, &reuse, false, &|_, _| Ok(None))?;
        assert_eq!(remote_oid(&seed, &identity.tag())?, canonical_oid);
        assert_eq!(remote_tags(&seed)?.len(), 5);
        assert!(reserve_validated(&seed, &reuse, false, &|_, _| Ok(None)).is_err());
        Ok(())
    }

    #[test]
    fn canonical_selection_reuses_source_and_rejects_changed_inputs_and_corruption() -> Result<()> {
        let (_temp, root, _) = failed_attempt_fixture()?;
        fs::write(root.join("gradle.properties"), "GROUP=com.example\n")?;
        git_text(&root, &["add", "."])?;
        git_text(&root, &["commit", "-m", "properties"])?;
        let source = git_text(&root, &["rev-parse", "HEAD"])?;
        let identity = Identity::new(&root, "demo", "1.2.3", &source)?;
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
        git_text(&root, &["push", "origin", "main", "--tags"])?;
        fs::write(root.join("unrelated.txt"), "docs only")?;
        git_text(&root, &["add", "."])?;
        git_text(&root, &["commit", "-m", "unrelated"])?;
        git_text(&root, &["push", "origin", "main"])?;
        let head = git_text(&root, &["rev-parse", "HEAD"])?;
        let tags = remote_tags(&root)?;
        for destination in ["maven-central", "jitpack"] {
            for requested in [None, Some("1.2.3")] {
                assert_eq!(
                    select(&root, "demo", &head, destination, &tags, requested)?,
                    ("1.2.3".into(), source.clone())
                );
            }
        }
        fs::write(root.join("gradle.properties"), "GROUP=com.changed\n")?;
        git_text(&root, &["add", "."])?;
        git_text(&root, &["commit", "-m", "build input change"])?;
        let changed = git_text(&root, &["rev-parse", "HEAD"])?;
        assert_eq!(
            select(&root, "demo", &changed, "jitpack", &tags, None)?.0,
            "1.2.4"
        );
        assert!(select(&root, "demo", &changed, "jitpack", &tags, Some("1.2.3")).is_err());
        let mut record = json!({"module":"demo", "version":"1.2.3", "source":source, "release_identity":identity});
        release_identity::validate_record(&root, &record)?;
        record["release_identity"]["fingerprint"] = json!("0".repeat(64));
        assert!(release_identity::validate_record(&root, &record).is_err());
        record["release_identity"] = serde_json::to_value(&identity)?;
        git_text(
            &root,
            &["push", "origin", &format!(":refs/tags/{}", identity.tag())],
        )?;
        assert!(release_identity::validate_record(&root, &record).is_err());
        assert!(select(&root, "demo", &head, "jitpack", &tags, None).is_err());
        git_text(
            &root,
            &[
                "tag",
                "-f",
                "-a",
                &identity.tag(),
                &changed,
                "-m",
                "malformed",
            ],
        )?;
        git_text(&root, &["push", "origin", "--tags"])?;
        assert!(release_identity::validate_record(&root, &record).is_err());
        Ok(())
    }

    #[test]
    fn legacy_other_destination_sets_shared_version_floor() -> Result<()> {
        let (_temp, root, source) = failed_attempt_fixture()?;
        fs::write(root.join("gradle.properties"), "GROUP=com.example\n")?;
        git_text(&root, &["add", "."])?;
        git_text(&root, &["commit", "-m", "properties"])?;
        git_text(&root, &["push", "origin", "main"])?;
        git_text(&root, &["tag", "demo/v2.0.0-jitpack", &source])?;
        git_text(&root, &["push", "origin", "--tags"])?;
        let head = git_text(&root, &["rev-parse", "HEAD"])?;
        let tags = remote_tags(&root)?;
        assert_eq!(
            select(&root, "demo", &head, "maven-central", &tags, None)?.0,
            "2.0.1"
        );
        assert!(select(&root, "demo", &head, "maven-central", &tags, Some("2.0.0")).is_err());
        Ok(())
    }

    #[test]
    fn release_identity_tracks_verification_config_and_repository_registry() -> Result<()> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        git_text(root, &["init"])?;
        configure_fixture_git(root)?;
        fs::create_dir_all(root.join(".project/setup"))?;
        fs::write(root.join("gradle.properties"), "GROUP=com.example\n")?;
        fs::write(
            root.join(".project/setup/config.yaml"),
            "verification: {}\n",
        )?;
        fs::write(
            root.join(".project/setup/repositories.yml"),
            "repositories: {}\n",
        )?;
        git_text(root, &["add", "."])?;
        git_text(root, &["commit", "-m", "fixture"])?;
        let original = git_text(root, &["rev-parse", "HEAD"])?;
        let mut previous = fingerprint(root, "demo", &original)?;
        for (path, content) in [
            (
                "config.yaml",
                "verification: {class_prefix: com/example/}\n",
            ),
            (
                "repositories.yml",
                "repositories: {jitpack: {publisher: jitpack}}\n",
            ),
        ] {
            fs::write(root.join(".project/setup").join(path), content)?;
            git_text(root, &["add", ".project/setup"])?;
            git_text(root, &["commit", "-m", "contract change"])?;
            let current = fingerprint(root, "demo", "HEAD")?;
            assert_ne!(previous, current, "{path}");
            previous = current;
        }
        Ok(())
    }

    #[test]
    fn central_metadata_checks_identity_and_distinguishes_absence() -> Result<()> {
        let path = format!("{CENTRAL}/com/example/demo/maven-metadata.xml");
        let absent = |url: &str, optional: bool| {
            assert_eq!(url, path);
            assert!(optional);
            Ok(None)
        };
        assert!(central_versions("com.example", "demo", &absent)?.is_empty());

        let found = |_: &str, _: bool| {
            Ok(Some(b"<metadata><groupId>com.example</groupId><artifactId>demo</artifactId><versioning><versions><version>1.2.3</version></versions></versioning></metadata>".to_vec()))
        };
        assert_eq!(central_versions("com.example", "demo", &found)?, ["1.2.3"]);
        assert!(central_versions("com.example", "wrong", &found).is_err());
        let failed = |_: &str, _: bool| anyhow::bail!("registry unavailable");
        assert!(central_versions("com.example", "demo", &failed).is_err());
        Ok(())
    }

    #[test]
    fn pending_archive_preserves_the_exact_tag_object() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("checkout");
        let remote = temp.path().join("origin.git");
        let remote_path = remote.to_str().context("Non-UTF-8 temporary path")?;
        fs::create_dir(&root)?;
        git_text(&root, &["init", "-b", "main"])?;
        git_text(&root, &["init", "--bare", remote_path])?;
        git_text(&root, &["remote", "add", "origin", remote_path])?;
        configure_fixture_git(&root)?;
        git_text(&root, &["commit", "--allow-empty", "-m", "fixture"])?;
        let pending = attempt("pending", "demo", "1.2.3", "jitpack");
        let failed = attempt("failed", "demo", "1.2.3", "jitpack");
        git_text(&root, &["tag", "-a", &pending, "-m", "test journal"])?;
        git_text(&root, &["push", "origin", &refname(&pending)])?;
        let original = remote_oid(&root, &pending)?.context("Missing pending tag")?;
        let wrong = git_text(&root, &["rev-parse", "HEAD"])?;
        assert!(archive_attempt(&root, &failed, &[(&pending, &wrong)]).is_err());
        assert_eq!(
            remote_oid(&root, &pending)?.as_deref(),
            Some(original.as_str())
        );
        assert_eq!(remote_oid(&root, &failed)?, None);
        archive_attempt(&root, &failed, &[(&pending, &original)])?;
        assert_eq!(remote_oid(&root, &pending)?, None);
        assert_eq!(remote_oid(&root, &failed)?, Some(original));
        Ok(())
    }

    #[test]
    fn upload_guard_and_abort_cannot_both_win() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let guard = temp.path().join("guard");
        let abort = temp.path().join("abort");
        let remote = temp.path().join("origin.git");
        let remote = remote.to_str().context("Non-UTF-8 temporary path")?;
        fs::create_dir(&guard)?;
        git_text(&guard, &["init", "-b", "main"])?;
        git_text(&guard, &["init", "--bare", remote])?;
        git_text(&guard, &["remote", "add", "origin", remote])?;
        configure_fixture_git(&guard)?;
        git_text(&guard, &["commit", "--allow-empty", "-m", "fixture"])?;
        git_text(&guard, &["push", "origin", "HEAD:refs/heads/main"])?;
        let source = git_text(&guard, &["rev-parse", "HEAD"])?;
        let module = "demo";
        let destination = "maven-central";
        let message = r#"{"phase":"reserved-before-upload"}"#;
        for version in ["1.2.3", "1.2.4"] {
            let pending = attempt("pending", module, version, destination);
            git_text(&guard, &["tag", "-a", &pending, "-m", message])?;
            git_text(&guard, &["push", "origin", &refname(&pending)])?;
        }
        git_text(temp.path(), &["clone", "--branch", "main", remote, "abort"])?;

        let version = "1.2.3";
        let pending = attempt("pending", module, version, destination);
        let marker = attempt("uploading", module, version, destination);
        let failed = attempt("failed", module, version, destination);
        let original = remote_oid(&guard, &pending)?.context("Missing pending tag")?;
        archive_attempt(&abort, &failed, &[(&pending, &original)])?;
        assert!(publish_guard(&guard, &pending, &original, &marker, &source).is_err());
        assert_eq!(remote_oid(&guard, &pending)?, None);
        assert_eq!(remote_oid(&guard, &marker)?, None);
        assert_eq!(remote_oid(&guard, &failed)?, Some(original));

        let version = "1.2.4";
        let pending = attempt("pending", module, version, destination);
        let marker = attempt("uploading", module, version, destination);
        let failed = attempt("failed", module, version, destination);
        let original = remote_oid(&guard, &pending)?.context("Missing pending tag")?;
        publish_guard(&guard, &pending, &original, &marker, &source)?;
        assert!(archive_attempt(&abort, &failed, &[(&pending, &original)]).is_err());
        assert_eq!(remote_oid(&guard, &failed)?, None);
        assert!(remote_oid(&guard, &marker)?.is_some());
        let renewed = remote_oid(&guard, &pending)?.context("Missing renewed pending tag")?;
        assert_ne!(original, renewed);
        assert_eq!(peel(&guard, &renewed)?, source);
        let tag = git_text(&guard, &["cat-file", "tag", &renewed])?;
        assert_eq!(
            tag.split_once("\n\n").context("Missing renewed journal")?.1,
            message
        );
        assert_eq!(
            git_text(&guard, &["rev-parse", &refname(&pending)])?,
            renewed
        );
        fetch_tags(&abort)?;
        assert_eq!(
            git_text(&abort, &["rev-parse", &refname(&pending)])?,
            renewed
        );
        Ok(())
    }

    #[test]
    fn simultaneous_central_reservations_leave_one_winner() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let seed = temp.path().join("seed");
        let remote = temp.path().join("origin.git");
        let remote = remote.to_str().context("Non-UTF-8 temporary path")?;
        fs::create_dir(&seed)?;
        git_text(&seed, &["init", "-b", "main"])?;
        git_text(&seed, &["init", "--bare", remote])?;
        git_text(&seed, &["remote", "add", "origin", remote])?;
        configure_fixture_git(&seed)?;
        fs::create_dir_all(seed.join(".project/setup"))?;
        fs::write(
            seed.join(".project/setup/config.yaml"),
            "repository: example/demo\nmodules:\n  demo: Demo\n",
        )?;
        fs::write(
            seed.join(".project/setup/repositories.yml"),
            "repositories:\n  maven-central:\n    environment: MavenCentral\n    publisher: central\n",
        )?;
        let group = "com.example.demo";
        let module = "demo";
        let version = "1.2.3";
        let destination = "maven-central";
        fs::write(
            seed.join("gradle.properties"),
            format!("GROUP={group}\nPOM_ARTIFACT_ID={module}\n"),
        )?;
        git_text(&seed, &["add", ".project/setup", "gradle.properties"])?;
        git_text(&seed, &["commit", "-m", "fixture"])?;
        git_text(&seed, &["push", "origin", "HEAD:refs/heads/main"])?;
        let source = git_text(&seed, &["rev-parse", "HEAD"])?;
        let mut checkouts = Vec::new();
        for name in ["left", "right"] {
            git_text(temp.path(), &["clone", "--branch", "main", remote, name])?;
            let checkout = temp.path().join(name);
            configure_fixture_git(&checkout)?;
            let artifacts = checkout.join(format!(
                "build/verification-repository/com/example/demo/{module}/{version}"
            ));
            fs::create_dir_all(&artifacts)?;
            for (suffix, bytes) in SUFFIXES
                .into_iter()
                .zip(crate::confirmation::tests::artifacts(
                    group, module, version,
                ))
            {
                fs::write(artifacts.join(format!("{module}-{version}{suffix}")), bytes)?;
            }
            let record = serde_json::to_value(check_local(
                &checkout,
                module,
                version,
                &source,
                destination,
                "same-run",
            )?)?;
            checkouts.push((checkout, record));
        }

        let ready = std::sync::Barrier::new(2);
        let central = |url: &str, optional: bool| -> Result<Option<Vec<u8>>> {
            assert!(optional);
            if url.ends_with(".module") {
                ready.wait();
            }
            Ok(None)
        };
        let mut winners = Vec::new();
        std::thread::scope(|threads| -> Result<()> {
            let reservations: Vec<_> = checkouts
                .iter()
                .map(|(checkout, record)| {
                    threads.spawn(|| reserve_validated(checkout, record, false, &central))
                })
                .collect();
            for (attempt, (checkout, _)) in reservations.into_iter().zip(&checkouts) {
                if attempt
                    .join()
                    .map_err(|_| anyhow::anyhow!("Reservation thread panicked"))?
                    .is_ok()
                {
                    winners.push(checkout);
                }
            }
            Ok(())
        })?;
        assert_eq!(winners.len(), 1);
        let stable = canonical(module, version, destination);
        let pending = attempt("pending", module, version, destination);
        assert_eq!(
            remote_tags(&seed)?.keys().collect::<BTreeSet<_>>(),
            [&stable, &pending, &release_identity::tag(module, version)]
                .into_iter()
                .collect(),
        );
        let (journal, _) = read_reservation(winners[0], module, version, destination, "")?;
        assert_eq!(journal["workflow_run"], "same-run");
        verify_stable(winners[0], &journal)?;
        Ok(())
    }

    #[test]
    fn failed_attempt_proof_checks_every_http_artifact_between_status_reads() -> Result<()> {
        use std::io::{BufRead, BufReader};
        use std::time::{Duration, Instant};
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let source = "a".repeat(40);
        let status = json!({"status":"Error", "commit":source}).to_string();
        let server = std::thread::spawn(move || -> Result<Vec<String>> {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut paths = Vec::new();
            while paths.len() < SUFFIXES.len() + 3 && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                let mut reader = BufReader::new(stream.try_clone()?);
                let mut request = String::new();
                reader.read_line(&mut request)?;
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .context("Missing request path")?
                    .to_owned();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line)?;
                    if line.trim().is_empty() {
                        break;
                    }
                }
                let (code, body) = if path.contains("/api/builds/") {
                    ("200 OK", status.as_str())
                } else {
                    ("404 Not Found", "")
                };
                write!(
                    stream,
                    "HTTP/1.1 {code}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )?;
                paths.push(path);
            }
            Ok(paths)
        });
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(None)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(2)))
            .build()
            .into();
        let fetch = |url: &str, optional: bool| -> Result<Option<Vec<u8>>> {
            let mut response = agent.get(&url.replacen(JITPACK, &endpoint, 1)).call()?;
            match response.status().as_u16() {
                200 => Ok(Some(response.body_mut().read_to_vec()?)),
                404 if optional => Ok(None),
                code => anyhow::bail!("Unexpected HTTP {code}"),
            }
        };
        let result = verify_failed_jitpack(
            &json!({"group":"com.github.example", "artifact":"demo", "consumer_version":"demo~v0.1.0-jitpack", "source":source}),
            &fetch,
        );
        let paths = server.join().expect("mock server panicked")?;
        result?;
        assert_eq!(paths.len(), SUFFIXES.len() + 3);
        assert_eq!(paths.first(), paths.last());
        assert!(paths[0].starts_with("/api/builds/"));
        for (path, suffix) in paths[1..paths.len() - 1]
            .iter()
            .zip(SUFFIXES.into_iter().chain([".jar"]))
        {
            assert!(path.ends_with(suffix), "{path}");
        }
        Ok(())
    }

    pub(super) fn failed_attempt_fixture() -> Result<(tempfile::TempDir, std::path::PathBuf, String)>
    {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("checkout");
        fs::create_dir(&root)?;
        git_text(&root, &["init", "-b", "main"])?;
        configure_fixture_git(&root)?;
        let remote = temp.path().join("origin.git");
        git_text(&root, &["init", "--bare", remote.to_str().unwrap()])?;
        git_text(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )?;
        fs::create_dir_all(root.join(".project/setup"))?;
        // Windows checkouts may use CRLF; parsing must not depend on LF replacements.
        fs::write(
            root.join(".project/setup/config.yaml"),
            "repository: example/demo\r\nmodules:\r\n  demo: Demo\r\n",
        )?;
        fs::write(
            root.join(".project/setup/repositories.yml"),
            "repositories:\r\n  maven-central:\r\n    environment: maven-central\r\n    publisher: central\r\n  jitpack:\r\n    environment: jitpack\r\n    publisher: jitpack\r\n",
        )?;
        git_text(&root, &["add", "."])?;
        git_text(&root, &["commit", "-m", "fixture"])?;
        git_text(&root, &["push", "origin", "main"])?;
        let source = git_text(&root, &["rev-parse", "HEAD"])?;
        Ok((temp, root, source))
    }

    pub(super) fn add_failed_attempt(root: &Path, version: &str, source: &str) -> Result<String> {
        add_jitpack_attempt(root, version, source, false)
    }

    fn add_jitpack_attempt(
        root: &Path,
        version: &str,
        source: &str,
        legacy: bool,
    ) -> Result<String> {
        let stable = if legacy {
            format!("demo/v{version}")
        } else {
            canonical("demo", version, "jitpack")
        };
        git_text(root, &["tag", &stable, source])?;
        let mut record = json!({
            "module": "demo", "version": version, "source": source,
            "group": "com.github.example", "artifact": "demo",
            "repository": "jitpack", "repository_url": JITPACK,
            "consumer_version": format!("demo~v{version}-jitpack"),
            "source_tag": stable, "phase": "reserved-before-upload",
            "sha256": SUFFIXES.into_iter().map(|suffix| (suffix, "a".repeat(64))).collect::<BTreeMap<_,_>>()
        });
        if legacy {
            record.as_object_mut().unwrap().remove("source_tag");
            record["consumer_version"] = json!(format!("demo~v{version}"));
        }
        let pending = attempt("pending", "demo", version, "jitpack");
        git_text(
            root,
            &["tag", "-a", &pending, source, "-m", &record.to_string()],
        )?;
        git_text(root, &["push", "origin", "--tags"])?;
        git_text(root, &["rev-parse", &refname(&pending)])
    }

    pub(super) fn failed_response(url: &str, source: &str) -> Result<Option<Vec<u8>>> {
        if url.contains("/api/builds/") {
            Ok(Some(serde_json::to_vec(
                &json!({"status":"Error", "commit":source}),
            )?))
        } else {
            Ok(None)
        }
    }

    #[test]
    fn automatic_recovery_archives_older_legacy_failure_after_newer_confirmation() -> Result<()> {
        let (_temp, root, source) = failed_attempt_fixture()?;
        let pending_oid = add_jitpack_attempt(&root, "0.1.0", &source, true)?;
        let (mut confirmed, _) = read_reservation(&root, "demo", "0.1.0", "jitpack", "jitpack/")?;
        confirmed["version"] = json!("0.1.1");
        confirmed["consumer_version"] = json!("demo~v0.1.1-jitpack");
        confirmed["source_tag"] = json!("demo/v0.1.1-jitpack");
        confirmed["phase"] = json!("confirmed-public");
        fs::create_dir_all(root.join("docs/releases/jitpack"))?;
        let metadata_path = root.join("docs/releases/jitpack/demo.json");
        fs::write(&metadata_path, serde_json::to_vec_pretty(&confirmed)?)?;
        git_text(&root, &["add", "docs"])?;
        git_text(&root, &["commit", "-m", "newer confirmed release"])?;
        git_text(&root, &["tag", "demo/v0.1.1-jitpack", &source])?;
        git_text(&root, &["push", "origin", "main", "--tags"])?;
        let corrected = git_text(&root, &["rev-parse", "HEAD"])?;
        let before_metadata = fs::read(&metadata_path)?;
        let before_tags = remote_tags(&root)?;
        let fetch = |url: &str, _: bool| failed_response(url, &source);
        recover_failed_jitpack(&root, "demo", &before_tags, "0.1.2", &corrected, &fetch)?;
        let mut expected_tags = before_tags;
        expected_tags.remove("release-pending/jitpack/demo/0.1.0");
        expected_tags.insert("release-failed/jitpack/demo/0.1.0".into(), pending_oid);
        assert_eq!(remote_tags(&root)?, expected_tags);
        assert_eq!(fs::read(&metadata_path)?, before_metadata);
        Ok(())
    }

    #[test]
    fn reconciliation_archives_legacy_tag_without_rewriting_release_identity() -> Result<()> {
        let (_temp, root, source) = failed_attempt_fixture()?;
        let oid = add_jitpack_attempt(&root, "0.1.0", &source, true)?;
        let before = remote_tags(&root)?;
        let fetch = |url: &str, _: bool| failed_response(url, &source);
        let preview = reconcile_failed_jitpack_with(&root, "demo", &[], false, &fetch)?;
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0].action, "eligible", "{}", preview[0].detail);
        assert_eq!(remote_tags(&root)?, before);
        let applied = reconcile_failed_jitpack_with(&root, "demo", &[], true, &fetch)?;
        assert_eq!(applied[0].action, "archived", "{}", applied[0].detail);
        let mut expected = before;
        expected.remove("release-pending/jitpack/demo/0.1.0");
        expected.insert("release-failed/jitpack/demo/0.1.0".into(), oid);
        assert_eq!(remote_tags(&root)?, expected);
        Ok(())
    }

    #[test]
    fn automatic_recovery_preserves_archived_confirmation_without_latest_record() -> Result<()> {
        for filename in ["0.1.0-jitpack.json", "0.1.0.json"] {
            let (_temp, root, source) = failed_attempt_fixture()?;
            add_failed_attempt(&root, "0.1.0", &source)?;
            let (mut confirmed, _) =
                read_reservation(&root, "demo", "0.1.0", "jitpack", "jitpack/")?;
            confirmed["phase"] = json!("confirmed-public");
            fs::create_dir_all(root.join("docs/releases/history/demo"))?;
            fs::write(
                root.join("docs/releases/history/demo").join(filename),
                serde_json::to_vec(&json!({
                    "schema": 1, "module": "demo", "version": "0.1.0", "source": source,
                    "documentation_ref": source, "destinations": {"jitpack": confirmed}
                }))?,
            )?;
            git_text(&root, &["add", "docs"])?;
            git_text(&root, &["commit", "-m", "archived confirmation"])?;
            git_text(&root, &["push", "origin", "main"])?;
            let corrected = git_text(&root, &["rev-parse", "HEAD"])?;
            let before = remote_tags(&root)?;
            let fetch = |url: &str, _: bool| failed_response(url, &source);
            assert!(
                recover_failed_jitpack(&root, "demo", &before, "0.1.1", &corrected, &fetch)
                    .is_err()
            );
            assert_eq!(remote_tags(&root)?, before);
        }
        Ok(())
    }

    #[test]
    fn automatic_recovery_reports_unknown_legacy_attempt_and_explicit_recovery() -> Result<()> {
        let (_temp, root, source) = failed_attempt_fixture()?;
        add_jitpack_attempt(&root, "0.1.0", &source, true)?;
        git_text(
            &root,
            &["commit", "--allow-empty", "-m", "corrected source"],
        )?;
        git_text(&root, &["push", "origin", "main"])?;
        let corrected = git_text(&root, &["rev-parse", "HEAD"])?;
        let before = remote_tags(&root)?;
        let fetch = |_: &str, _: bool| -> Result<Option<Vec<u8>>> {
            Ok(Some(serde_json::to_vec(&json!({
                "version": "demo~v0.1.0", "status": "tagNotFound", "message": "",
                "commit": "", "modules": [], "isTag": false
            }))?))
        };
        let error = recover_failed_jitpack(&root, "demo", &before, "0.1.1", &corrected, &fetch)
            .expect_err("Unknown build status must preserve the journal");
        let message = format!("{error:#}");
        for detail in [
            "tagNotFound",
            "release-pending/jitpack/demo/0.1.0",
            "--version X.Y.Z",
            "--allow-pending-jitpack",
            "bypass_version_verification",
        ] {
            assert!(message.contains(detail), "Missing {detail}: {message}");
        }
        assert_eq!(remote_tags(&root)?, before);
        Ok(())
    }

    #[test]
    fn automatic_reconciliation_preserves_a_replaced_valid_pending_journal() -> Result<()> {
        let (_temp, root, source) = failed_attempt_fixture()?;
        let original = add_failed_attempt(&root, "0.1.0", &source)?;
        let pending = "release-pending/jitpack/demo/0.1.0";
        let inspected = BTreeMap::from([(pending.to_owned(), original)]);
        let (mut replacement, _) = read_reservation(&root, "demo", "0.1.0", "jitpack", "jitpack/")?;
        replacement["workflow_run"] = json!("replacement-run");
        git_text(
            &root,
            &[
                "tag",
                "-f",
                "-a",
                pending,
                &source,
                "-m",
                &replacement.to_string(),
            ],
        )?;
        git_text(&root, &["push", "--force", "origin", &refname(pending)])?;
        let before = remote_tags(&root)?;
        let fetch = |url: &str, _: bool| failed_response(url, &source);
        let report = reconcile_failed_jitpack_snapshot(
            &root,
            "demo",
            &["0.1.0".into()],
            true,
            &fetch,
            &inspected,
        )?;
        assert_eq!(report[0].action, "needs-review");
        assert_eq!(remote_tags(&root)?, before);
        Ok(())
    }

    #[test]
    fn reconciliation_previews_filters_archives_exact_objects_and_is_repeatable() -> Result<()> {
        let (_temp, root, source) = failed_attempt_fixture()?;
        let first = add_failed_attempt(&root, "0.1.0", &source)?;
        let second = add_failed_attempt(&root, "0.1.1", &source)?;
        // A newer successful release must not block cleaning older failed attempts.
        fs::create_dir_all(root.join("docs/releases/jitpack"))?;
        fs::write(
            root.join("docs/releases/jitpack/demo.json"),
            r#"{"version":"0.1.2"}"#,
        )?;
        git_text(&root, &["add", "docs"])?;
        git_text(&root, &["commit", "-m", "newer confirmed release"])?;
        git_text(&root, &["push", "origin", "main"])?;
        let before = remote_tags(&root)?;
        let fetch = |url: &str, _: bool| failed_response(url, &source);
        let preview = reconcile_failed_jitpack_with(&root, "demo", &[], false, &fetch)?;
        assert_eq!(preview.len(), 2);
        assert!(
            preview.iter().all(|entry| entry.action == "eligible"),
            "{}",
            serde_json::to_string(&preview)?
        );
        assert_eq!(remote_tags(&root)?, before);
        let selected =
            reconcile_failed_jitpack_with(&root, "demo", &["0.1.0".into()], true, &fetch)?;
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].action, "archived");
        let remaining = reconcile_failed_jitpack_with(&root, "demo", &[], true, &fetch)?;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].action, "archived");
        for (version, oid) in [("0.1.0", first), ("0.1.1", second)] {
            assert_eq!(
                remote_oid(&root, &attempt("failed", "demo", version, "jitpack"))?,
                Some(oid)
            );
            assert!(remote_oid(&root, &attempt("pending", "demo", version, "jitpack"))?.is_none());
            assert_eq!(
                remote_oid(&root, &canonical("demo", version, "jitpack"))?,
                Some(source.clone())
            );
        }
        assert!(reconcile_failed_jitpack_with(&root, "demo", &[], true, &fetch)?.is_empty());
        Ok(())
    }

    #[test]
    fn reconciliation_preserves_active_wrong_source_public_and_uncertain_attempts() -> Result<()> {
        let (_temp, root, source) = failed_attempt_fixture()?;
        add_failed_attempt(&root, "0.1.0", &source)?;
        let before = remote_tags(&root)?;
        for scenario in [
            "building",
            "wrong-source",
            "artifact",
            "network",
            "status-changed",
        ] {
            let statuses = std::cell::Cell::new(0);
            let fetch = |url: &str, _: bool| -> Result<Option<Vec<u8>>> {
                if scenario == "network" {
                    anyhow::bail!("unavailable")
                }
                if url.contains("/api/builds/") {
                    statuses.set(statuses.get() + 1);
                    let status = if scenario == "building"
                        || (scenario == "status-changed" && statuses.get() > 1)
                    {
                        "Building"
                    } else {
                        "Error"
                    };
                    let commit = if scenario == "wrong-source" {
                        "b".repeat(40)
                    } else {
                        source.clone()
                    };
                    Ok(Some(serde_json::to_vec(
                        &json!({"status":status,"commit":commit}),
                    )?))
                } else if scenario == "artifact" && url.ends_with("-sources.jar") {
                    Ok(Some(vec![1]))
                } else {
                    Ok(None)
                }
            };
            let report = reconcile_failed_jitpack_with(&root, "demo", &[], true, &fetch)?;
            assert_eq!(report[0].action, "needs-review", "{scenario}");
            assert_eq!(remote_tags(&root)?, before, "{scenario}");
        }
        Ok(())
    }

    #[test]
    fn reconciliation_preserves_upload_markers_and_confirmed_history() -> Result<()> {
        let (_temp, root, source) = failed_attempt_fixture()?;
        add_failed_attempt(&root, "0.1.0", &source)?;
        add_failed_attempt(&root, "0.1.1", &source)?;
        add_failed_attempt(&root, "0.1.2", &source)?;
        git_text(
            &root,
            &[
                "tag",
                &attempt("uploading", "demo", "0.1.0", "jitpack"),
                &source,
            ],
        )?;
        git_text(&root, &["push", "origin", "--tags"])?;
        fs::create_dir_all(root.join("docs/releases/history/demo"))?;
        fs::create_dir_all(root.join("docs/releases/jitpack"))?;
        fs::write(
            root.join("docs/releases/history/demo/0.1.1-jitpack.json"),
            r#"{"destinations":{"jitpack":{}}}"#,
        )?;
        fs::write(
            root.join("docs/releases/jitpack/demo.json"),
            r#"{"version":"0.1.2"}"#,
        )?;
        git_text(&root, &["add", "docs"])?;
        git_text(&root, &["commit", "-m", "confirmed records"])?;
        git_text(&root, &["push", "origin", "main"])?;
        let before = remote_tags(&root)?;
        let fetch = |url: &str, _: bool| failed_response(url, &source);
        let report = reconcile_failed_jitpack_with(&root, "demo", &[], true, &fetch)?;
        assert_eq!(report.len(), 3);
        assert!(report.iter().all(|entry| entry.action == "needs-review"));
        assert_eq!(remote_tags(&root)?, before);
        Ok(())
    }

    #[test]
    fn reconciliation_atomic_lease_preserves_replaced_pending_journal() -> Result<()> {
        let (_temp, root, source) = failed_attempt_fixture()?;
        add_failed_attempt(&root, "0.1.0", &source)?;
        let pending = attempt("pending", "demo", "0.1.0", "jitpack");
        let statuses = std::cell::Cell::new(0);
        let fetch = |url: &str, _: bool| -> Result<Option<Vec<u8>>> {
            if url.contains("/api/builds/") {
                statuses.set(statuses.get() + 1);
                if statuses.get() == 2 {
                    git_text(
                        &root,
                        &[
                            "tag",
                            "-f",
                            "-a",
                            &pending,
                            &source,
                            "-m",
                            "concurrent reservation",
                        ],
                    )?;
                    git_text(&root, &["push", "--force", "origin", &refname(&pending)])?;
                }
            }
            failed_response(url, &source)
        };
        let report = reconcile_failed_jitpack_with(&root, "demo", &[], true, &fetch)?;
        assert_eq!(report[0].action, "needs-review");
        assert_eq!(
            remote_oid(&root, &pending)?,
            Some(git_text(&root, &["rev-parse", &refname(&pending)])?)
        );
        assert!(remote_oid(&root, &attempt("failed", "demo", "0.1.0", "jitpack"))?.is_none());
        Ok(())
    }

    fn configure_fixture_git(root: &Path) -> Result<()> {
        git_text(root, &["config", "user.name", "Journal Fixture"])?;
        git_text(root, &["config", "user.email", "fixture@example.invalid"])?;
        git_text(root, &["config", "commit.gpgSign", "false"])?;
        git_text(root, &["config", "tag.gpgSign", "false"])?;
        Ok(())
    }
}
