use crate::confirmation::git_text;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub schema: u32,
    pub module: String,
    pub version: String,
    pub source: String,
    pub fingerprint: String,
}

impl Identity {
    pub(crate) fn new(root: &Path, module: &str, version: &str, source: &str) -> Result<Self> {
        Ok(Self {
            schema: 1,
            module: module.into(),
            version: version.into(),
            source: source.into(),
            fingerprint: fingerprint(root, module, source)?,
        })
    }
    pub(crate) fn tag(&self) -> String {
        tag(&self.module, &self.version)
    }
}

pub(crate) fn tag(module: &str, version: &str) -> String {
    format!("release-identity/{module}/{version}")
}

pub(crate) fn read(root: &Path, module: &str, version: &str) -> Result<Option<Identity>> {
    let reference = format!("refs/tags/{}", tag(module, version));
    let remote = git_text(root, &["ls-remote", "--refs", "origin", &reference])?;
    if remote.is_empty() {
        return Ok(None);
    }
    let (oid, found) = remote.split_once('\t').context("Malformed identity ref")?;
    ensure!(found == reference, "Unexpected identity ref");
    ensure!(
        git_text(root, &["rev-parse", "--verify", &reference])? == oid,
        "Canonical identity tag moved; fetch and investigate"
    );
    ensure!(
        git_text(root, &["cat-file", "-t", oid])? == "tag",
        "Canonical identity must be annotated"
    );
    let object = git_text(root, &["cat-file", "-p", oid])?;
    let (_, message) = object
        .split_once("\n\n")
        .context("Missing canonical identity")?;
    let identity: Identity =
        serde_json::from_str(message).context("Malformed canonical identity")?;
    ensure!(
        identity.schema == 1 && identity.module == module && identity.version == version,
        "Canonical identity metadata mismatch"
    );
    ensure!(
        identity.source.len() == 40
            && identity
                .source
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid canonical source"
    );
    ensure!(
        git_text(root, &["rev-parse", &format!("{oid}^{{commit}}")])? == identity.source,
        "Canonical identity source tag mismatch"
    );
    ensure!(
        identity.fingerprint == fingerprint(root, module, &identity.source)?,
        "Canonical identity fingerprint mismatch"
    );
    Ok(Some(identity))
}

pub(crate) fn validate_record(root: &Path, record: &Value) -> Result<()> {
    let module = record["module"]
        .as_str()
        .context("Missing identity module")?;
    let version = record["version"]
        .as_str()
        .context("Missing identity version")?;
    let canonical = read(root, module, version)?;
    match record.get("release_identity") {
        Some(value) => {
            let identity: Identity = serde_json::from_value(value.clone())?;
            ensure!(
                record["source"] == identity.source
                    && identity.module == module
                    && identity.version == version,
                "Publication identity differs from record"
            );
            ensure!(
                canonical.as_ref() == Some(&identity),
                "Missing or conflicting canonical identity tag"
            );
        }
        None => ensure!(
            canonical.is_none(),
            "Canonical release requires embedded identity"
        ),
    }
    Ok(())
}
pub(crate) fn fingerprint(root: &Path, module: &str, source: &str) -> Result<String> {
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
