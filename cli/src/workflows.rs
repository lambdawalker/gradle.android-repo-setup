use anyhow::{bail, ensure, Context, Result};
use fs2::FileExt;
use regex::{Captures, Regex};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use similar::TextDiff;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

macro_rules! templates {
    ($($name:literal),+ $(,)?) => {
        &[$(($name, include_str!(concat!("../templates/", $name, ".j2")))),+]
    };
}

const TEMPLATES: &[(&str, &str)] = templates!(
    "build-signed-demo-apk.yml",
    "documentation.yml",
    "finalize-release.yml",
    "reconcile-failed-jitpack.yml",
    "publish-demo-common.yml",
    "publish-jitpack.yml",
    "publish-library.yml",
    "screenshots.yml",
    "unit-tests.yml",
    "update-workflows.yml",
    "verify-release-tooling.yml",
);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Check,
    Generate,
    Overwrite,
    OnlyMissing,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default, rename = "verification")]
    _verification: Option<crate::release::VerificationConfig>,
    repository: String,
    tooling_ref: String,
    workflows: WorkflowConfig,
    #[serde(default, rename = "title")]
    _title: Option<String>,
    #[serde(default, rename = "modules")]
    _modules: BTreeMap<String, String>,
    #[serde(default, rename = "key_alias")]
    _key_alias: Option<String>,
}

#[derive(Deserialize)]
struct WorkflowConfig {
    module: String,
    #[serde(default = "default_app_module")]
    app_module: String,
    title: String,
    #[serde(default = "default_screenshot_count")]
    screenshot_count: u32,
    #[serde(
        default = "default_java_version",
        deserialize_with = "deserialize_java_version"
    )]
    java_version: String,
    #[serde(default = "default_documentation_command")]
    documentation_command: String,
    #[serde(default = "default_screenshot_check_command")]
    screenshot_check_command: String,
    #[serde(default = "default_release_validation_command")]
    release_validation_command: String,
}

fn default_release_validation_command() -> String {
    "true".into()
}

fn default_screenshot_count() -> u32 {
    5
}
fn default_java_version() -> String {
    "21".into()
}
fn deserialize_java_version<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Version {
        Number(u16),
        Text(String),
    }
    Ok(match Version::deserialize(deserializer)? {
        Version::Number(value) => value.to_string(),
        Version::Text(value) => value,
    })
}
fn default_documentation_command() -> String {
    "android-repo-setup build_docs".into()
}
fn default_screenshot_check_command() -> String {
    "cd docs/screenshots && sha256sum -c images.sha256".into()
}

fn default_app_module() -> String {
    "sample".into()
}

fn render(root: &Path) -> Result<Vec<(&'static str, String)>> {
    let path = root.join(".project/setup/config.yaml");
    reject_symlink(&path)?;
    let config: Config = serde_yaml_ng::from_str(&read(&path)?)
        .with_context(|| format!("Invalid configuration in {}", path.display()))?;
    ensure!(
        Regex::new(r"^[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9_.-]+$")?.is_match(&config.repository)
            && !matches!(config.repository.split('/').next_back(), Some("." | "..")),
        "Repository must have owner/name form"
    );
    validate_tooling_ref(&config.tooling_ref)?;
    ensure!(
        Regex::new(r"^[A-Za-z0-9_-][A-Za-z0-9_.-]*$")?.is_match(&config.workflows.module),
        "workflows.module must be a single Gradle module name"
    );
    ensure!(
        Regex::new(r"^[A-Za-z0-9_-][A-Za-z0-9_.-]*$")?.is_match(&config.workflows.app_module),
        "workflows.app_module must be a single Gradle module name"
    );
    if let Some(contract) = &config._verification {
        contract.validate()?;
    }
    ensure!(
        config
            .workflows
            .java_version
            .parse::<u16>()
            .is_ok_and(|version| version >= 8),
        "Invalid workflows.java_version"
    );
    let screenshot_count = config.workflows.screenshot_count.to_string();
    let java_version = config.workflows.java_version.to_string();
    // JSON strings are valid YAML scalars, preserving shell punctuation and newlines.
    let documentation_command = serde_json::to_string(&config.workflows.documentation_command)?;
    let screenshot_check_command =
        serde_json::to_string(&config.workflows.screenshot_check_command)?;
    let release_validation_command =
        serde_json::to_string(&config.workflows.release_validation_command)?;
    ensure!(
        !config.workflows.documentation_command.trim().is_empty()
            && !config.workflows.screenshot_check_command.trim().is_empty()
            && !config
                .workflows
                .release_validation_command
                .trim()
                .is_empty(),
        "Workflow commands must not be empty"
    );
    let title = &config.workflows.title;
    ensure!(
        !title.trim().is_empty()
            && !title.contains('\'')
            && !title.contains("${{")
            && !title.chars().any(char::is_control),
        "workflows.title must be one line without single quotes or GitHub expressions"
    );

    let placeholders = Regex::new(r"<<@\s*([a-z_.]+)\s*@>>")?;
    TEMPLATES
        .iter()
        .map(|&(name, template)| {
            let rendered = placeholders
                .replace_all(template, |captures: &Captures<'_>| match &captures[1] {
                    "tooling_ref" => config.tooling_ref.as_str(),
                    "workflows.module" => config.workflows.module.as_str(),
                    "workflows.app_module" => config.workflows.app_module.as_str(),
                    "workflows.title" => title.as_str(),
                    "workflows.screenshot_count" => screenshot_count.as_str(),
                    "workflows.java_version" => java_version.as_str(),
                    "workflows.documentation_command" => documentation_command.as_str(),
                    "workflows.screenshot_check_command" => screenshot_check_command.as_str(),
                    "workflows.release_validation_command" => release_validation_command.as_str(),
                    _ => "<<@ unknown @>>",
                })
                .into_owned();
            ensure!(
                !rendered.contains("<<@"),
                "Unresolved placeholder in {name}"
            );
            let rendered = crate::publishing::render_workflow(root, name, &rendered)?;
            let _: serde_yaml_ng::Value = serde_yaml_ng::from_str(&rendered)
                .with_context(|| format!("Rendered workflow {name} is invalid YAML"))?;
            Ok((name, rendered))
        })
        .collect()
}

fn validate_tooling_ref(reference: &str) -> Result<()> {
    ensure!(
        Regex::new(r"\A(?:v[0-9]+\.[0-9]+\.[0-9]+|[0-9a-f]{40})\z")?.is_match(reference),
        "tooling_ref must be a release tag or full commit SHA"
    );
    Ok(())
}

fn set_tooling_ref(root: &Path, reference: &str) -> Result<()> {
    validate_tooling_ref(reference)?;
    let project = root.join(".project");
    let setup = project.join("setup");
    let path = setup.join("config.yaml");
    for entry in [&project, &setup, &path] {
        reject_symlink(entry)?;
    }
    let content = fs::read_to_string(&path)?;
    let key = Regex::new(r"(?m)^tooling_ref:[^\r\n]*")?;
    ensure!(
        key.find_iter(&content).count() == 1,
        "Expected one top-level tooling_ref"
    );
    let updated = key.replace(&content, format!("tooling_ref: '{reference}'"));
    let config: Config = serde_yaml_ng::from_str(&updated)?;
    ensure!(
        config.tooling_ref == reference,
        "Could not update tooling_ref"
    );
    replace(&path, &updated)
}

pub(crate) fn update(root: &Path, mode: Mode) -> Result<bool> {
    update_with_ref(root, mode, None)
}

pub(crate) fn update_with_ref(root: &Path, mode: Mode, reference: Option<&str>) -> Result<bool> {
    ensure!(
        reference.is_none() || mode == Mode::Overwrite,
        "Updating the tooling pin requires overwrite mode"
    );
    let project = root.join(".project");
    let setup = project.join("setup");
    let github = root.join(".github");
    let output = github.join("workflows");
    let lock = setup.join("workflow-generation.lock");
    let manifest = setup.join("workflow-generation.json");
    for path in [&project, &setup, &github, &output, &lock, &manifest] {
        reject_symlink(path)?;
    }
    if mode == Mode::Check {
        return compare_and_apply(root, &output, &manifest, mode);
    }

    let lock_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock)
        .with_context(|| format!("Could not open {}", lock.display()))?;
    FileExt::lock_exclusive(&lock_file)?;
    if let Some(reference) = reference {
        set_tooling_ref(root, reference)?;
    }
    compare_and_apply(root, &output, &manifest, mode)
}

fn compare_and_apply(root: &Path, output: &Path, manifest: &Path, mode: Mode) -> Result<bool> {
    let mut hashes: BTreeMap<String, String> = match read_optional(manifest)? {
        Some(text) => serde_json::from_str(&text).context("Invalid workflow hash manifest")?,
        None => BTreeMap::new(),
    };
    let mut changes: Vec<(&str, PathBuf, String, bool)> = Vec::new();
    for (name, content) in render(root)? {
        let path = output.join(name);
        let previous = read_optional(&path)?;
        if previous.as_deref() == Some(content.as_str())
            || (mode == Mode::OnlyMissing && previous.is_some())
        {
            continue;
        }
        let kind = match &previous {
            Some(text) if hashes.get(name) != Some(&hash(text)) => "local/untracked changes",
            _ => "template update",
        };
        setup_message!("{name}: {kind}");
        setup_message!(
            "{}",
            TextDiff::from_lines(previous.as_deref().unwrap_or(""), &content)
                .unified_diff()
                .header(name, &format!("{name} (generated)"))
        );
        changes.push((name, path, content, previous.is_some()));
    }
    if mode == Mode::Check {
        return Ok(!changes.is_empty());
    }
    if mode == Mode::Generate && changes.iter().any(|entry| entry.3) {
        bail!("Existing workflows differ. Review the diff and pass --overwrite to replace them.");
    }

    fs::create_dir_all(output)?;
    for (name, path, content, _) in &changes {
        replace(path, content)?;
        hashes.insert((*name).to_owned(), hash(content));
    }
    if !changes.is_empty() || !manifest.exists() {
        replace(manifest, &serde_json::to_string_pretty(&hashes)?)?;
    }
    Ok(!changes.is_empty())
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map(|text| text.replace("\r\n", "\n").replace('\r', "\n"))
        .with_context(|| format!("Could not read {}", path.display()))
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    reject_symlink(path)?;
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.replace("\r\n", "\n").replace('\r', "\n"))),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Could not read {}", path.display())),
    }
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            !metadata.file_type().is_symlink(),
            "Refusing a symbolic link at {}",
            path.display()
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn replace(path: &Path, content: &str) -> Result<()> {
    reject_symlink(path)?;
    let mut staged = NamedTempFile::new_in(path.parent().context("Missing parent directory")?)?;
    staged.write_all(content.as_bytes())?;
    staged
        .persist(path)
        .with_context(|| format!("Could not replace {}", path.display()))?;
    Ok(())
}

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
