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
    // Resolve the source layout from the immutable revision, not today's checkout.
    let config_output = crate::release::git(
        root,
        &["show", &format!("{source}:.project/setup/config.yaml")],
    )?;
    let config: serde_yaml_ng::Value = if config_output.status.success() {
        serde_yaml_ng::from_slice(&config_output.stdout)?
    } else {
        serde_yaml_ng::Value::Null
    };
    let directory = config["android_modules"][module]["directory"]
        .as_str()
        .unwrap_or(module);
    let excluded: Vec<String> = if config["fingerprint_excluded_properties"][module].is_null() {
        vec![]
    } else {
        serde_yaml_ng::from_value(config["fingerprint_excluded_properties"][module].clone())?
    };
    let root_jvm =
        config["android_modules"][module]["kind"] == "jvm-library" && matches!(directory, "." | "");
    let local = [
        "src/main",
        "build.gradle.kts",
        "README.md",
        "docs",
        "consumer-rules.pro",
        "proguard-rules.pro",
    ]
    .into_iter()
    .filter(|path| !root_jvm || *path != "docs")
    .map(|path| format!("{}/{path}", directory.trim_end_matches('/')))
    .collect::<Vec<_>>();
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
        ".project/setup/config.yaml",
        ".project/verification",
        ".project/setup/repositories.yml",
        ".github/workflows/publish-library.yml",
        ".github/workflows/publish-jitpack.yml",
    ]);
    if !root_jvm {
        args.push("docs/releases.md");
    }
    let groovy = format!("{}/build.gradle", directory.trim_end_matches('/'));
    if !config["android_modules"].is_null() {
        args.extend([
            groovy.as_str(),
            "build.gradle",
            "settings.gradle",
            "buildSrc",
            "build-logic",
        ]);
    }
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
    let mut properties = git_text(root, &["show", &format!("{source}:gradle.properties")])?;
    if !excluded.is_empty() {
        properties = properties
            .lines()
            .filter(|line| {
                !line
                    .split_once('=')
                    .is_some_and(|(key, _)| excluded.iter().any(|name| name == key.trim()))
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(format!("{entries}\n{}", properties.trim()))
    ))
}

#[cfg(test)]
mod module_tests {
    use super::*;
    #[test]
    fn root_jvm_release_identity_excludes_generated_documentation() -> Result<()> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        std::fs::create_dir_all(root.join(".project/setup"))?;
        std::fs::create_dir_all(root.join("src/main"))?;
        std::fs::write(
            root.join(".project/setup/config.yaml"),
            "android_modules:\n  coordinates: {path: ':', directory: '.', kind: jvm-library}\n",
        )?;
        std::fs::write(root.join("src/main/Point.kt"), "class Point")?;
        std::fs::write(root.join("gradle.properties"), "GROUP=com.example\n")?;
        git_text(root, &["init", "-b", "main"])?;
        git_text(root, &["config", "user.name", "Test"])?;
        git_text(root, &["config", "user.email", "test@example.invalid"])?;
        let commit = || -> Result<String> {
            git_text(root, &["add", "."])?;
            git_text(
                root,
                &["-c", "commit.gpgSign=false", "commit", "-m", "fixture"],
            )?;
            git_text(root, &["rev-parse", "HEAD"])
        };
        let original = fingerprint(root, "coordinates", &commit()?)?;
        std::fs::create_dir_all(root.join("docs/releases"))?;
        std::fs::create_dir_all(root.join("docs/es"))?;
        std::fs::write(root.join("docs/releases/coordinates.json"), "confirmation")?;
        std::fs::write(root.join("docs/es/index.md"), "translation")?;
        std::fs::write(root.join("IMPORT.md"), "installation")?;
        assert_eq!(original, fingerprint(root, "coordinates", &commit()?)?);
        std::fs::write(root.join("src/main/Point.kt"), "class ChangedPoint")?;
        assert_ne!(original, fingerprint(root, "coordinates", &commit()?)?);
        Ok(())
    }

    #[test]
    fn fingerprints_follow_remapped_sources_and_ignore_only_configured_properties() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        std::fs::create_dir_all(root.join(".project/setup"))?;
        std::fs::create_dir_all(root.join("components/vision/src/main"))?;
        std::fs::write(root.join(".project/setup/config.yaml"), "android_modules:\n  libs--vision: {path: ':libs:vision', directory: components/vision, kind: library}\nfingerprint_excluded_properties:\n  libs--vision: [modelCoreVersion]\n")?;
        std::fs::write(
            root.join("gradle.properties"),
            "GROUP=com.example\nmodelCoreVersion=1.0.0\n",
        )?;
        std::fs::write(root.join("components/vision/src/main/Main.kt"), "first")?;
        git_text(root, &["init"])?;
        git_text(root, &["config", "user.name", "Test"])?;
        git_text(root, &["config", "user.email", "test@example.invalid"])?;
        let commit = || -> Result<String> {
            git_text(root, &["add", "."])?;
            git_text(
                root,
                &["-c", "commit.gpgSign=false", "commit", "-m", "fixture"],
            )?;
            git_text(root, &["rev-parse", "HEAD"])
        };
        let first = commit()?;
        let original = fingerprint(root, "libs--vision", &first)?;
        std::fs::write(
            root.join("gradle.properties"),
            "GROUP=com.example\nmodelCoreVersion=2.0.0\n",
        )?;
        let second = commit()?;
        assert_eq!(original, fingerprint(root, "libs--vision", &second)?);
        std::fs::write(root.join("components/vision/src/main/Main.kt"), "second")?;
        let third = commit()?;
        assert_ne!(original, fingerprint(root, "libs--vision", &third)?);
        assert_eq!(original, fingerprint(root, "libs--vision", &first)?);
        let before_groovy = fingerprint(root, "libs--vision", &third)?;
        std::fs::write(
            root.join("components/vision/build.gradle"),
            "dependencies { api 'example:core:2.0.0' }",
        )?;
        let fourth = commit()?;
        assert_ne!(before_groovy, fingerprint(root, "libs--vision", &fourth)?);
        Ok(())
    }
}
