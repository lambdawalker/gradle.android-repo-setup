mod contracts;
use anyhow::{ensure, Context, Result};
use contracts::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Cursor, Read};
use std::path::Path;
use std::process::Command;
use zip::ZipArchive;

pub(super) const SUFFIXES: [&str; 5] = [".pom", ".aar", "-sources.jar", "-javadoc.jar", ".module"];
pub(super) const CENTRAL: &str = "https://repo.maven.apache.org/maven2";
pub(super) const JITPACK: &str = "https://jitpack.io";

#[derive(Deserialize)]
struct ProjectConfig {
    repository: String,
    modules: BTreeMap<String, String>,
}

#[derive(Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Packaging {
    #[default]
    Aar,
    Jar,
}
impl Packaging {
    fn is_default(&self) -> bool {
        *self == Self::Aar
    }
    fn extension(&self) -> &'static str {
        match self {
            Self::Aar => ".aar",
            Self::Jar => ".jar",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct VerificationConfig {
    #[serde(skip_serializing_if = "Packaging::is_default")]
    pub packaging: Packaging,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jvm_version: Option<u16>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub require_kotlin_stdlib: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forbidden_dependency_group_prefixes: Vec<String>,
    pub class_prefix: String,
    pub required_api_artifacts: Vec<String>,
    pub bytecode_major: u16,
    pub required_api_dependencies: Vec<DependencyPin>,
    pub required_assets: Option<AssetSource>,
    pub assets: Vec<crate::assets::AssetSet>,
    #[serde(skip_deserializing)]
    pub asset_sizes: BTreeMap<String, u64>,
    #[serde(skip_deserializing)]
    pub asset_rules: BTreeMap<String, crate::assets::AssetRules>,
    pub forbidden_asset_suffixes: Vec<String>,
    #[serde(skip_deserializing)]
    pub asset_sha256: BTreeMap<String, String>,
}

impl Default for VerificationConfig {
    fn default() -> Self {
        Self {
            packaging: Packaging::Aar,
            jvm_version: None,
            require_kotlin_stdlib: false,
            forbidden_dependency_group_prefixes: Vec::new(),
            class_prefix: "com/apexfission/android/democommon/".into(),
            required_api_artifacts: vec!["foundation".into(), "ui".into()],
            bytecode_major: 61,
            required_api_dependencies: Vec::new(),
            required_assets: None,
            assets: Vec::new(),
            asset_sizes: BTreeMap::new(),
            asset_rules: BTreeMap::new(),
            forbidden_asset_suffixes: Vec::new(),
            asset_sha256: BTreeMap::new(),
        }
    }
}

impl VerificationConfig {
    pub(crate) fn suffixes(&self) -> [&'static str; 5] {
        [
            ".pom",
            self.packaging.extension(),
            "-sources.jar",
            "-javadoc.jar",
            ".module",
        ]
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            Regex::new(r"\A(?:[A-Za-z_$][A-Za-z0-9_$]*/)+\z")?.is_match(&self.class_prefix),
            "verification.class_prefix must be a slash-separated package prefix ending in /"
        );
        ensure!(
            self.bytecode_major >= 45,
            "Invalid verification.bytecode_major"
        );
        ensure!(
            self.required_api_artifacts
                .iter()
                .all(|id| !id.trim().is_empty()),
            "verification.required_api_artifacts must contain nonempty artifact IDs"
        );
        ensure!(
            self.jvm_version.is_none_or(|version| version > 0),
            "Invalid verification.jvm_version"
        );
        ensure!(
            self.forbidden_dependency_group_prefixes
                .iter()
                .all(|prefix| !prefix.trim().is_empty()),
            "Forbidden dependency group prefixes must be nonempty"
        );
        validate_contract(self)?;
        Ok(())
    }
}

pub(crate) fn verification_config(
    root: &Path,
    source: &str,
    module: &str,
) -> Result<VerificationConfig> {
    let output = git(
        root,
        &["show", &format!("{source}:.project/setup/config.yaml")],
    )?;
    ensure!(
        output.status.success(),
        "Could not read .project/setup/config.yaml at immutable source {source}. \
         If this release predates the CLI, verify its reserved artifacts with the original release tooling \
         at that source, then use the current finalizer with the verified confirmation record. \
         Preserve pending/uploading markers and never re-upload the version. \
         Do not substitute the current checkout's verification contract."
    );
    #[derive(Deserialize)]
    struct Config {
        #[serde(default)]
        verification: VerificationConfig,
        #[serde(default)]
        module_verification: BTreeMap<String, VerificationConfig>,
    }
    let config: Config = serde_yaml_ng::from_slice(&output.stdout)?;
    let mut contract = config
        .module_verification
        .into_iter()
        .find(|(id, _)| id == module)
        .map(|(_, value)| value)
        .unwrap_or(config.verification);
    let source_config: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&output.stdout)?;
    if source_config["android_modules"][module]["kind"] == "jvm-library" {
        ensure!(
            contract.packaging == Packaging::Jar,
            "JVM library requires an explicit verification.packaging: jar contract"
        );
    }
    resolve_contract(root, source, &mut contract)?;
    contract.validate()?;
    Ok(contract)
}

pub(crate) fn reserved_verification(
    root: &Path,
    record: &serde_json::Value,
) -> Result<VerificationConfig> {
    let Some(schema) = record.get("verification_schema") else {
        return verification_config(
            root,
            record["source"].as_str().context("Missing source")?,
            record["module"].as_str().context("Missing module")?,
        );
    };
    ensure!(
        schema == 1,
        "Unsupported reserved verification schema; preserve the journal"
    );
    let value = &record["verification"];
    let fields = value
        .as_object()
        .context("Missing reserved verification snapshot")?;
    let defaults = serde_json::to_value(VerificationConfig::default())?;
    ensure!(
        defaults
            .as_object()
            .unwrap()
            .keys()
            .all(|name| fields.contains_key(name)),
        "Incomplete reserved verification snapshot"
    );
    let mut configured = value.clone();
    for key in ["asset_sha256", "asset_sizes", "asset_rules"] {
        configured.as_object_mut().unwrap().remove(key);
    }
    let mut contract: VerificationConfig = serde_json::from_value(configured)?;
    // These derived maps must not be supplied by setup YAML, but are required
    // when restoring the immutable resolved reservation, including model hashes.
    contract.asset_sha256 = serde_json::from_value(value["asset_sha256"].clone())?;
    contract.asset_sizes = serde_json::from_value(value["asset_sizes"].clone())?;
    contract.asset_rules = serde_json::from_value(value["asset_rules"].clone())?;
    ensure!(
        contract
            .required_api_dependencies
            .iter()
            .all(|pin| pin.group_property.is_none()
                && pin.artifact_property.is_none()
                && pin.version_property.is_none()),
        "Reserved dependency pins must be resolved"
    );
    contract.validate()?;
    Ok(contract)
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    #[test]
    fn snapshot_retains_resolved_assets_without_source_config() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut original = VerificationConfig::default();
        original
            .asset_sha256
            .insert("assets/model.bin".into(), "a".repeat(64));
        original.asset_sizes.insert("assets/model.bin".into(), 42);
        original.asset_rules.insert(
            "assets/model.bin".into(),
            crate::assets::AssetRules {
                validator: crate::assets::Validator::default(),
                allow_empty_files: false,
            },
        );
        let record = serde_json::json!({"verification_schema":1,"verification":original,"source":"a".repeat(40),"module":"demo"});
        let recovered = reserved_verification(root.path(), &record)?;
        assert_eq!(serde_json::to_value(recovered)?, record["verification"]);
        let mut unsupported = record.clone();
        unsupported["verification_schema"] = serde_json::json!(2);
        assert!(reserved_verification(root.path(), &unsupported).is_err());
        let mut incomplete = record;
        incomplete["verification"]
            .as_object_mut()
            .unwrap()
            .remove("asset_sha256");
        assert!(reserved_verification(root.path(), &incomplete).is_err());
        Ok(())
    }
}

#[derive(Serialize)]
pub(crate) struct Record<'a> {
    release_identity: crate::release_identity::Identity,
    module: &'a str,
    version: &'a str,
    source: &'a str,
    group: String,
    artifact: String,
    sha256: BTreeMap<&'static str, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    content_sha256: BTreeMap<&'static str, BTreeMap<String, String>>,
    verification: VerificationConfig,
    verification_schema: u32,
    upload_protocol: &'static str,
    workflow_attempt: String,
    phase: &'static str,
    workflow_run: &'a str,
    repository: &'a str,
    repository_url: &'static str,
    source_tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    consumer_version: Option<String>,
}

pub(crate) fn check_local<'a>(
    root: &Path,
    module: &'a str,
    version: &'a str,
    source: &'a str,
    destination: &'a str,
    workflow_run: &'a str,
) -> Result<Record<'a>> {
    ensure!(
        Regex::new(r"\A[0-9a-f]{40}\z")?.is_match(source),
        "Invalid source SHA"
    );
    let head = git(root, &["rev-parse", "HEAD"])?;
    ensure!(
        head.status.success() && String::from_utf8(head.stdout)?.trim() == source,
        "Checkout differs from source"
    );
    let diff = git(
        root,
        &["diff", "--quiet", "--no-ext-diff", source, "--", "."],
    )?;
    ensure!(
        diff.status.success(),
        "Tracked files differ from source or Git could not verify the checkout"
    );

    let contract = verification_config(root, source, module)?;
    verify_published_dependencies(&contract, &crate::confirmation::registry_fetch(CENTRAL))?;
    let coordinates = coordinates(root, module, version, destination, true)?;
    let Coordinates {
        group,
        artifact,
        repository_url,
        consumer_version,
    } = coordinates;
    let consumer = consumer_version.as_deref().unwrap_or(version);
    let directory = root
        .join("build/verification-repository")
        .join(group.replace('.', "/"))
        .join(&artifact)
        .join(consumer);
    let mut sha256 = BTreeMap::new();
    let mut content_sha256 = BTreeMap::new();
    for suffix in contract.suffixes() {
        let path = directory.join(format!("{artifact}-{consumer}{suffix}"));
        let data = fs::read(&path).with_context(|| format!("Could not read {}", path.display()))?;
        verify(&data, suffix, &group, &artifact, consumer, true, &contract)
            .with_context(|| format!("Invalid artifact {}", path.display()))?;
        sha256.insert(suffix, format!("{:x}", Sha256::digest(&data)));
        if contract.packaging == Packaging::Jar && matches!(suffix, "-sources.jar" | "-javadoc.jar")
        {
            content_sha256.insert(suffix, jar_contents(&data)?);
        }
    }
    Ok(Record {
        release_identity: crate::release_identity::Identity::new(root, module, version, source)?,
        module,
        version,
        source,
        group,
        artifact,
        sha256,
        content_sha256,
        verification: contract,
        verification_schema: 1,
        upload_protocol: "guard-v1",
        workflow_attempt: std::env::var("GITHUB_RUN_ATTEMPT").unwrap_or_else(|_| "local".into()),
        phase: "reserved-before-upload",
        workflow_run,
        repository: destination,
        repository_url,
        source_tag: format!("{module}/v{version}-{destination}"),
        consumer_version,
    })
}

pub(super) struct Coordinates {
    pub group: String,
    pub artifact: String,
    pub repository_url: &'static str,
    pub consumer_version: Option<String>,
}

pub(super) fn coordinates(
    root: &Path,
    module: &str,
    version: &str,
    destination: &str,
    scoped: bool,
) -> Result<Coordinates> {
    coordinates_for_record(root, module, version, destination, scoped, true, None)
}

pub(super) fn coordinates_for_record(
    root: &Path,
    module: &str,
    version: &str,
    destination: &str,
    scoped: bool,
    require_configured: bool,
    record_artifact: Option<&str>,
) -> Result<Coordinates> {
    ensure!(
        Regex::new(r"\A(?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*)){2}\z")?.is_match(version),
        "Expected stable X.Y.Z"
    );
    let config: ProjectConfig = serde_yaml_ng::from_str(&fs::read_to_string(
        root.join(".project/setup/config.yaml"),
    )?)?;
    ensure!(
        config.modules.contains_key(module)
            && Regex::new(r"\A[A-Za-z0-9_-][A-Za-z0-9_.-]*\z")?.is_match(module),
        "Unknown or invalid release module"
    );
    let (owner, repo) = config
        .repository
        .split_once('/')
        .context("Expected repository owner/name")?;
    let slug_part = Regex::new(r"\A[A-Za-z0-9_.-]+\z")?;
    for part in [owner, repo] {
        ensure!(
            slug_part.is_match(part) && !matches!(part, "." | ".."),
            "Invalid repository identity"
        );
    }
    if require_configured {
        if let Some(modules) = crate::discovery::load(root)? {
            let selected = modules
                .get(module)
                .context("Unknown configured Android module")?;
            ensure!(
                selected.selected && selected.kind.is_library(),
                "Release module is not a selected library"
            );
            let expected = crate::discovery::inventory(root, &modules)?;
            ensure!(
                fs::read_to_string(root.join(".project/setup/modules.properties"))? == expected,
                "Module inventory is stale; run android-repo-setup workflows --overwrite"
            );
        }
        let registry =
            crate::publishing::load_registry(&root.join(".project/setup/repositories.yml"))?;
        ensure!(
            registry.contains_key(destination),
            "Unknown release repository"
        );
    }

    if destination == "jitpack" {
        ensure!(
            record_artifact.is_none_or(|artifact| artifact == repo),
            "Unexpected JitPack module/artifact identity"
        );
        return Ok(Coordinates {
            group: format!("com.github.{owner}"),
            artifact: repo.to_owned(),
            repository_url: JITPACK,
            consumer_version: Some(format!(
                "{module}~v{version}{}",
                if scoped { "-jitpack" } else { "" }
            )),
        });
    }
    let (group, artifact) = local_identity(root, module)?;
    Ok(Coordinates {
        group,
        artifact,
        repository_url: CENTRAL,
        consumer_version: None,
    })
}

pub(super) use android_repo_docs::local_identity;

pub(super) fn git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .context("Could not run Git")
}

// zip stores entries by name internally, so duplicate central-directory records
// can disappear before by_index() iteration. Count raw records as well.
fn verify_zip_directory(data: &[u8], archive: &ZipArchive<Cursor<&[u8]>>) -> Result<()> {
    let mut offset = usize::try_from(archive.central_directory_start())?;
    let mut count = 0;
    while data.get(offset..offset + 4) == Some(b"PK\x01\x02") {
        let header = data
            .get(offset..offset + 46)
            .context("Truncated ZIP directory")?;
        let length = |at| usize::from(u16::from_le_bytes([header[at], header[at + 1]]));
        offset = offset
            .checked_add(46 + length(28) + length(30) + length(32))
            .context("ZIP directory overflow")?;
        ensure!(offset <= data.len(), "Truncated ZIP directory entry");
        count += 1;
    }
    ensure!(count == archive.len(), "Duplicate archive entries");
    Ok(())
}

pub(super) fn jar_contents(data: &[u8]) -> Result<BTreeMap<String, String>> {
    let mut archive = ZipArchive::new(Cursor::new(data))?;
    verify_zip_directory(data, &archive)?;
    let mut names = HashSet::new();
    let mut contents = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        ensure!(
            names.insert(name.clone()),
            "Duplicate archive entry: {name}"
        );
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if !entry.is_dir() && name != "META-INF/MANIFEST.MF" {
            contents.insert(name, format!("{:x}", Sha256::digest(&bytes)));
        }
    }
    Ok(contents)
}

pub(super) fn verify(
    data: &[u8],
    suffix: &str,
    group: &str,
    artifact: &str,
    version: &str,
    local: bool,
    contract: &VerificationConfig,
) -> Result<()> {
    ensure!(
        contract.suffixes().contains(&suffix),
        "Artifact suffix does not match verification packaging: {suffix}"
    );
    if suffix == ".pom" {
        return verify_pom(data, group, artifact, version, contract);
    }
    if suffix == ".module" {
        let metadata: serde_json::Value = serde_json::from_slice(data)?;
        let component = &metadata["component"];
        ensure!(
            ["group", "module", "version"].map(|key| component[key].as_str())
                == [Some(group), Some(artifact), Some(version)],
            "Gradle module coordinates mismatch"
        );
        verify_module_dependencies(&metadata, contract)?;
        return Ok(());
    }

    let mut archive = ZipArchive::new(Cursor::new(data))?;
    verify_zip_directory(data, &archive)?;
    let mut names = HashSet::new();
    let mut classes = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        ensure!(
            names.insert(name.clone()),
            "Duplicate archive entry: {name}"
        );
        if suffix == ".aar" && name.starts_with("assets/") && !entry.is_dir() {
            ensure!(
                !contract
                    .forbidden_asset_suffixes
                    .iter()
                    .any(|suffix| name.ends_with(suffix)),
                "Forbidden AAR asset: {name}"
            );
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            crate::assets::verify_archive_entry(contract, &name, &bytes)?;
        }
        if suffix == ".jar" && name.ends_with(".class") && name.starts_with(&contract.class_prefix)
        {
            ensure!(
                !name.contains("DemoPreviews"),
                "Debug fixture must not be included"
            );
            let mut header = [0; 8];
            entry.read_exact(&mut header)?;
            ensure!(
                header[..4] == [0xca, 0xfe, 0xba, 0xbe]
                    && u16::from_be_bytes([header[6], header[7]]) == contract.bytecode_major,
                "Unexpected JVM bytecode target"
            );
            classes.push(1);
        }
        if suffix == ".aar" && name == "classes.jar" {
            entry.read_to_end(&mut classes)?;
        } else {
            io::copy(&mut entry, &mut io::sink())?;
        }
        if local && suffix == "-javadoc.jar" && !entry.is_dir() {
            ensure!(
                name.ends_with(".md")
                    || (contract.packaging == Packaging::Jar && name.ends_with(".html"))
                    || matches!(name.as_str(), "LICENSE" | "META-INF/MANIFEST.MF"),
                "Unexpected documentation archive content: {name}"
            );
        }
    }
    match suffix {
        ".jar" => ensure!(!classes.is_empty(), "Missing library classes"),
        ".aar" => {
            if let Some(assets) = &contract.required_assets {
                ensure!(
                    contract
                        .asset_sha256
                        .keys()
                        .any(|name| name.starts_with("assets/") && name.ends_with(&assets.suffix)),
                    "Required AAR asset set is empty"
                );
            }
            for name in contract.asset_sha256.keys() {
                ensure!(names.contains(name), "Missing required AAR asset: {name}");
            }
            ensure!(
                names.contains("AndroidManifest.xml") && names.contains("classes.jar"),
                "AAR is missing manifest or classes.jar"
            );
            let mut jar = ZipArchive::new(Cursor::new(classes))?;
            let mut found = false;
            for index in 0..jar.len() {
                let mut entry = jar.by_index(index)?;
                let name = entry.name();
                if name.ends_with(".class") && name.starts_with(&contract.class_prefix) {
                    ensure!(
                        !name.contains("DemoPreviews"),
                        "Debug fixture must not be included"
                    );
                    let mut header = [0; 8];
                    entry.read_exact(&mut header)?;
                    ensure!(
                        header[..4] == [0xca, 0xfe, 0xba, 0xbe]
                            && u16::from_be_bytes([header[6], header[7]])
                                == contract.bytecode_major,
                        "Unexpected JVM bytecode target"
                    );
                    io::copy(&mut entry, &mut io::sink())?;
                    found = true;
                }
            }
            ensure!(found, "Missing library classes");
        }
        "-sources.jar" => ensure!(
            names.iter().any(|name| name.ends_with(".kt")
                || (contract.packaging == Packaging::Jar && name.ends_with(".java"))),
            "Sources JAR has no supported library source"
        ),
        "-javadoc.jar" => ensure!(
            names.iter().any(|name| name.ends_with(".md")
                || ((!local || contract.packaging == Packaging::Jar) && name.ends_with(".html"))),
            "Documentation JAR has no documentation"
        ),
        _ => unreachable!(),
    }
    Ok(())
}

fn verify_pom(
    data: &[u8],
    group: &str,
    artifact: &str,
    version: &str,
    contract: &VerificationConfig,
) -> Result<()> {
    let xml = std::str::from_utf8(data)?;
    let upper = xml.to_ascii_uppercase();
    ensure!(
        !upper.contains("<!DOCTYPE") && !upper.contains("<!ENTITY"),
        "XML DTD and entity declarations are not allowed"
    );
    let document = roxmltree::Document::parse(xml)?;
    let root = document.root_element();
    let value = |path: &str| {
        path.split('/')
            .try_fold(root, |node, tag| {
                node.children().find(|child| child.has_tag_name(tag))
            })
            .and_then(|node| node.text())
    };
    ensure!(
        [value("groupId"), value("artifactId"), value("version")]
            == [Some(group), Some(artifact), Some(version)],
        "POM coordinates/version mismatch"
    );
    ensure!(
        value("packaging").unwrap_or("jar") == &contract.packaging.extension()[1..],
        "POM packaging does not match verification contract"
    );
    for path in [
        "name",
        "description",
        "url",
        "licenses/license/name",
        "licenses/license/url",
        "developers/developer/id",
        "developers/developer/name",
        "scm/url",
        "scm/connection",
    ] {
        ensure!(
            value(path).is_some_and(|text| !text.is_empty()),
            "Missing POM metadata: {path}"
        );
    }
    verify_pom_dependencies(root, contract)?;
    let mut exported = HashSet::new();
    if let Some(dependencies) = root
        .children()
        .find(|node| node.has_tag_name("dependencies"))
    {
        for dep in dependencies
            .children()
            .filter(|node| node.has_tag_name("dependency"))
        {
            let text = |key| {
                dep.children()
                    .find(|node| node.has_tag_name(key))
                    .and_then(|node| node.text())
            };
            ensure!(
                text("artifactId") != Some("unspecified") && text("version") != Some("unspecified"),
                "Unresolved project dependency in POM"
            );
            if text("scope").unwrap_or("compile") == "compile" {
                if let Some(id) = text("artifactId") {
                    exported.insert(id);
                }
            }
        }
    }
    for id in &contract.required_api_artifacts {
        ensure!(
            exported.contains(id.as_str()),
            "Required API artifact must be exported: {id}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_source_without_contract_reports_recovery_and_rejects_current_contract() -> Result<()>
    {
        let project = tempfile::tempdir()?;
        let root = project.path();
        for args in [
            vec!["init"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "legacy source",
            ],
        ] {
            ensure!(
                git(root, &args)?.status.success(),
                "Fixture Git command failed"
            );
        }
        let source = crate::confirmation::git_text(root, &["rev-parse", "HEAD"])?;
        fs::create_dir_all(root.join(".project/setup"))?;
        fs::write(
            root.join(".project/setup/config.yaml"),
            "verification:\n  class_prefix: com/example/current/\n  required_api_artifacts: []\n",
        )?;
        let error = verification_config(root, &source, "demo")
            .unwrap_err()
            .to_string();
        assert!(error.contains(&source), "{error}");
        assert!(error.contains("original release tooling"), "{error}");
        assert!(error.contains("never re-upload"), "{error}");
        Ok(())
    }

    #[test]
    fn verification_contract_is_read_from_the_immutable_source() -> Result<()> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        fs::create_dir_all(root.join(".project/setup"))?;
        let path = root.join(".project/setup/config.yaml");
        fs::write(&path, "verification:\n  class_prefix: com/example/permissions/\n  required_api_artifacts: [permissions]\n")?;
        for args in [
            vec!["init"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "fixture",
            ],
        ] {
            ensure!(
                git(root, &args)?.status.success(),
                "Fixture Git command failed"
            );
        }
        fs::write(&path, "verification:\n  class_prefix: wrong/checkout/\n")?;
        let contract = verification_config(root, "HEAD", "demo")?;
        assert_eq!(contract.class_prefix, "com/example/permissions/");
        assert_eq!(contract.required_api_artifacts, vec!["permissions"]);
        assert_eq!(contract.bytecode_major, 61);
        Ok(())
    }

    #[test]
    fn malformed_contracts_cannot_disable_package_verification() {
        for prefix in [
            "",
            "com.example",
            "/",
            "com/example",
            "com//example/",
            "../",
        ] {
            let contract = VerificationConfig {
                class_prefix: prefix.into(),
                ..Default::default()
            };
            assert!(contract.validate().is_err(), "{prefix}");
        }
        assert!(
            serde_yaml_ng::from_str::<VerificationConfig>("class_prefx: com/example/").is_err()
        );
    }

    #[test]
    fn module_coordinates_follow_the_gradle_artifact_precedence() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let properties = root.join("gradle.properties");
        fs::write(
            &properties,
            "GROUP=com.example.android\nPOM_ARTIFACT_ID=shared\nPOM_ARTIFACT_ID.demo=custom-demo\n",
        )
        .unwrap();
        assert_eq!(local_identity(root, "demo").unwrap().1, "custom-demo");
        assert_eq!(local_identity(root, "other").unwrap().1, "shared");
        fs::write(&properties, "GROUP=com.example.android\n").unwrap();
        assert_eq!(local_identity(root, "demo").unwrap().1, "demo");
    }
}
