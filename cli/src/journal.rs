use crate::confirmation::{
    git_text, read_reservation, registry_fetch, validate_record, validate_record_at,
};
use crate::release::{check_local, coordinates, git, CENTRAL, JITPACK, SUFFIXES};
use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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

fn fingerprint(root: &Path, module: &str, source: &str) -> Result<String> {
    let local = [
        "src/main",
        "build.gradle.kts",
        "README.md",
        "docs",
        "consumer-rules.pro",
        "proguard-rules.pro",
    ]
    .map(|path| format!("{module}/{path}"));
    let mut args = vec!["ls-tree", "-r", source, "--"];
    args.extend(local.iter().map(String::as_str));
    args.extend([
        "build.gradle.kts",
        "settings.gradle.kts",
        "gradle",
        "gradlew",
        "gradlew.bat",
        "LICENSE",
        "jitpack.yml",
        "scripts",
        "publishing",
        "docs/agents",
        "docs/releases.md",
        ".project/setup/config.yaml",
        ".project/setup/repositories.yml",
        ".github/workflows/publish-library.yml",
        ".github/workflows/publish-jitpack.yml",
    ]);
    let listing = git_text(root, &args)?;
    let entries = listing
        .lines()
        .filter(|line| {
            !line
                .split_once('\t')
                .is_some_and(|(_, path)| path.starts_with("scripts/tests/"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let properties = git_text(root, &["show", &format!("{source}:gradle.properties")])?;
    Ok(format!(
        "{:x}",
        Sha256::digest(format!("{entries}\n{}", properties.trim()))
    ))
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
    let mut versions = BTreeMap::new();
    let mut add = |version: &str, oid: &str| -> Result<()> {
        let commit = peel(root, oid)?;
        if let Some(old) = versions.insert(version.to_owned(), commit.clone()) {
            ensure!(old == commit, "Conflicting source identities for {module} {version}; reconcile history before publishing");
        }
        Ok(())
    };
    for (name, oid) in tags {
        if let Some(version) = history_version(name, module, destination) {
            add(version, oid)?;
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
    for path in listing
        .lines()
        .filter(|path| path.ends_with(".json") && !path.starts_with("docs/releases/history/"))
    {
        let Some(record) = remote_file(root, path)?.filter(|value| !value.is_null()) else {
            continue;
        };
        let recorded_module = field(&record, "module")?;
        let recorded_destination = record_destination(&record)?;
        let version = field(&record, "version")?;
        validate_record_at(
            root,
            recorded_module,
            version,
            recorded_destination,
            &record,
            false,
        )?;
        if recorded_module != module || recorded_destination != destination {
            continue;
        }
        let candidates = match record_tag(&record) {
            Some(name) => vec![name.to_owned()],
            None => legacy_tags(&record)?.to_vec(),
        };
        let recorded_source = field(&record, "source")?;
        let mut found = false;
        for candidate in candidates {
            if let Some(oid) = tags.get(&candidate) {
                if peel(root, oid)? == recorded_source {
                    add(version, oid)?;
                    found = true;
                    break;
                }
            }
        }
        ensure!(found, "Confirmed release is missing its source tag");
    }
    if let Some(requested) = requested {
        version_key(requested)?;
    }
    let Some((latest, original)) = versions
        .iter()
        .max_by_key(|(version, _)| version_key(version).ok())
    else {
        return Ok((requested.unwrap_or("0.1.0").to_owned(), source.to_owned()));
    };
    ancestor(root, original, source)
        .context("Latest module release must be an ancestor of selected source")?;
    let same = fingerprint(root, module, source)? == fingerprint(root, module, original)?;
    if let Some(requested) = requested {
        ensure!(
            compare(requested, latest)?.is_gt(),
            "Explicit version must be newer than the latest identity; use Finalize for recovery"
        );
        return Ok((requested.to_owned(), source.to_owned()));
    }
    if same {
        Ok((latest.clone(), original.clone()))
    } else {
        Ok((bump_patch(latest)?, source.to_owned()))
    }
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
    for suffix in SUFFIXES {
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
            "JitPack attempt is not a proven terminal failure; preserve pending journal"
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
    if let Some(current) = current(root, module, "jitpack")? {
        ensure!(
            compare(field(&current, "version")?, old_version)?.is_lt(),
            "Confirmed JitPack history requires manual reconciliation"
        );
    }
    verify_failed_jitpack(&record, fetch)?;
    archive_attempt(
        root,
        &attempt("failed", module, old_version, "jitpack"),
        &[(name, &oid)],
    )?;
    println!("Archived failed JitPack {module} {old_version}; canonical tag retained; advancing to {version}");
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
    coordinates(root, module, "0.1.0", "jitpack", true)?;
    for version in versions {
        version_key(version)?;
    }
    let tags = remote_tags(root)?;
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
            ensure!(
                record["source_tag"] == canonical(module, version, "jitpack"),
                "A canonical destination source tag is required"
            );
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
            recover_failed_jitpack(
                root,
                module,
                &tags,
                &version,
                &source,
                &registry_fetch(JITPACK),
            )?;
            ensure_available(root, module, destination, &remote_tags(root)?, None)?;
        }
    }
    if let Some(record) = current(root, module, destination)? {
        if record["version"] == version {
            validate_record(root, module, &version, destination, &record)?;
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
    let mut args = vec!["push", "--atomic", &pending_lease];
    let stable_oid = remote_oid(root, &stable)?;
    if let Some(oid) = &stable_oid {
        ensure!(
            peel(root, oid)? == source,
            "Canonical tag has different source"
        );
    } else {
        args.push(&stable_lease);
    }
    args.push("origin");
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
    if let Some(current) = current(root, module, destination)? {
        ensure!(
            compare(field(&current, "version")?, version)?.is_lt(),
            "Refusing to replace same/newer module metadata"
        );
    }
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

    git_text(
        root,
        &["switch", "-C", &format!("finalize-{module}"), "origin/main"],
    )?;
    crate::docs::seed(root)?;
    crate::docs::archive_record(root, &record)?;
    let metadata = metadata(module, destination);
    let path = root.join(&metadata);
    fs::create_dir_all(
        path.parent()
            .context("Missing release metadata directory")?,
    )?;
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&record)?),
    )?;
    crate::docs::documentation(root, false)?;
    git_text(root, &["add", &metadata, "IMPORT.md", crate::docs::HISTORY])?;
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
    let coords = coordinates(root, module, version, destination, true)?;
    fetch_tags(root)?;
    if let Some(current) = current(root, module, destination)? {
        ensure!(
            current["version"] != version,
            "Version already finalized at destination; cannot abort"
        );
    }
    let pending = attempt("pending", module, version, destination);
    let marker = attempt("uploading", module, version, destination);
    let pending_oid = remote_oid(root, &pending)?;
    let marker_oid = remote_oid(root, &marker)?;
    let mut refs = Vec::new();
    if let Some(oid) = &pending_oid {
        let (record, checked_oid) =
            read_reservation(root, module, version, destination, scope(destination))?;
        ensure!(*oid == checked_oid, "Remote reservation differs");
        if let Some(marker_oid) = &marker_oid {
            ensure!(
                peel(root, marker_oid)? == field(&record, "source")?,
                "Upload marker source differs from reservation"
            );
        }
        refs.push((pending.as_str(), oid.as_str()));
    }
    if let Some(oid) = &marker_oid {
        refs.push((marker.as_str(), oid.as_str()));
    }
    ensure!(
        !refs.is_empty(),
        "No unconfirmed release attempt found for this version"
    );
    let published = central_versions(&coords.group, &coords.artifact, &registry_fetch(CENTRAL))?;
    ensure!(
        !published.iter().any(|published| published == version),
        "Version is published on Maven Central; run finalize instead of aborting"
    );
    let archive = attempt("failed", module, version, destination);
    archive_attempt(root, &archive, &refs)?;
    println!("Archived unconfirmed {module} {version} to {archive}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            [&stable, &pending].into_iter().collect(),
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
            while paths.len() < SUFFIXES.len() + 2 && Instant::now() < deadline {
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
        assert_eq!(paths.len(), SUFFIXES.len() + 2);
        assert_eq!(paths.first(), paths.last());
        assert!(paths[0].starts_with("/api/builds/"));
        for (path, suffix) in paths[1..paths.len() - 1].iter().zip(SUFFIXES) {
            assert!(path.ends_with(suffix), "{path}");
        }
        Ok(())
    }

    fn failed_attempt_fixture() -> Result<(tempfile::TempDir, std::path::PathBuf, String)> {
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

    fn add_failed_attempt(root: &Path, version: &str, source: &str) -> Result<String> {
        let stable = canonical("demo", version, "jitpack");
        git_text(root, &["tag", &stable, source])?;
        let record = json!({
            "module": "demo", "version": version, "source": source,
            "group": "com.github.example", "artifact": "demo",
            "repository": "jitpack", "repository_url": JITPACK,
            "consumer_version": format!("demo~v{version}-jitpack"),
            "source_tag": stable, "phase": "reserved-before-upload",
            "sha256": SUFFIXES.into_iter().map(|suffix| (suffix, "a".repeat(64))).collect::<BTreeMap<_,_>>()
        });
        let pending = attempt("pending", "demo", version, "jitpack");
        git_text(
            root,
            &["tag", "-a", &pending, source, "-m", &record.to_string()],
        )?;
        git_text(root, &["push", "origin", "--tags"])?;
        git_text(root, &["rev-parse", &refname(&pending)])
    }

    fn failed_response(url: &str, source: &str) -> Result<Option<Vec<u8>>> {
        if url.contains("/api/builds/") {
            Ok(Some(serde_json::to_vec(
                &json!({"status":"Error", "commit":source}),
            )?))
        } else {
            Ok(None)
        }
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
