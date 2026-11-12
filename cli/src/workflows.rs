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
    #[serde(default)]
    android_modules: Option<crate::discovery::Modules>,
    #[serde(default, rename = "module_verification")]
    _module_verification: BTreeMap<String, crate::release::VerificationConfig>,
    #[serde(default, rename = "fingerprint_excluded_properties")]
    _fingerprint_excluded_properties: BTreeMap<String, Vec<String>>,
    repository: String,
    tooling_ref: String,
    workflows: WorkflowConfig,
    #[serde(default, rename = "title")]
    _title: Option<String>,
    #[serde(default, rename = "modules")]
    _modules: BTreeMap<String, String>,
    #[serde(default, rename = "key_alias")]
    _key_alias: Option<String>,
    #[serde(default, rename = "signing")]
    _signing: Option<serde_yaml_ng::Value>,
}

#[derive(Deserialize)]
struct WorkflowConfig {
    #[serde(default)]
    module: String,
    #[serde(default = "default_app_module")]
    app_module: String,
    #[serde(default)]
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
    #[serde(default)]
    screenshot_check_command: Option<String>,
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

struct RenderTarget {
    name: String,
    template_name: &'static str,
    module: String,
    module_path: String,
    module_dir: String,
    app: String,
    app_path: String,
    app_dir: String,
}

fn render(root: &Path) -> Result<Vec<(String, String)>> {
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
    if let Some(modules) = &config.android_modules {
        crate::discovery::validate(modules)?;
    } else {
        ensure!(
            Regex::new(r"^[A-Za-z0-9_-][A-Za-z0-9_.-]*$")?.is_match(&config.workflows.module),
            "workflows.module must be a single Gradle module name"
        );
        ensure!(
            Regex::new(r"^[A-Za-z0-9_-][A-Za-z0-9_.-]*$")?.is_match(&config.workflows.app_module),
            "workflows.app_module must be a single Gradle module name"
        );
    }
    for contract in config._module_verification.values() {
        contract.validate()?;
    }
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
    let default_screenshot_check = if config.android_modules.is_some() {
        "true".to_owned()
    } else {
        default_screenshot_check_command()
    };
    let screenshot_check = config
        .workflows
        .screenshot_check_command
        .as_deref()
        .unwrap_or(&default_screenshot_check);
    let screenshot_check_command = serde_json::to_string(screenshot_check)?;
    let release_validation_command =
        serde_json::to_string(&config.workflows.release_validation_command)?;
    ensure!(
        !config.workflows.documentation_command.trim().is_empty()
            && !screenshot_check.trim().is_empty()
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

    let docs_runtime = documentation_runtime(root, false);
    let release_runtime = documentation_runtime(root, true);
    let placeholders = Regex::new(r"<<@\s*([a-z_.]+)\s*@>>")?;
    // One shared release engine, dedicated entry points, per-app APK workflows.
    let mut plans: Vec<RenderTarget> = Vec::new();
    let legacy_module = &config.workflows.module;
    let legacy_app = &config.workflows.app_module;
    if let Some(modules) = &config.android_modules {
        let libraries = modules
            .iter()
            .filter(|(_, m)| m.selected && m.kind == crate::discovery::Kind::Library)
            .collect::<Vec<_>>();
        let apps = modules
            .iter()
            .filter(|(_, m)| m.selected && m.kind == crate::discovery::Kind::Application)
            .collect::<Vec<_>>();
        ensure!(
            !libraries.is_empty() || !apps.is_empty(),
            "Select at least one Android module"
        );
        for &(template, _) in TEMPLATES {
            let targets: Vec<(&String, &crate::discovery::Module)> = match template {
                "publish-demo-common.yml"
                | "verify-release-tooling.yml"
                | "reconcile-failed-jitpack.yml" => libraries.clone(),
                "build-signed-demo-apk.yml" => apps.clone(),
                "screenshots.yml" => libraries
                    .iter()
                    .chain(apps.iter())
                    .copied()
                    .filter(|(_, m)| m.screenshots)
                    .collect(),
                _ => {
                    if libraries.is_empty()
                        && matches!(
                            template,
                            "publish-library.yml"
                                | "publish-jitpack.yml"
                                | "finalize-release.yml"
                                | "documentation.yml"
                        )
                    {
                        continue;
                    }
                    let (id, m) = libraries.first().or(apps.first()).unwrap();
                    plans.push(RenderTarget {
                        name: template.into(),
                        template_name: template,
                        module: (*id).clone(),
                        module_path: m.path.trim_end_matches(':').into(),
                        module_dir: m.directory.clone(),
                        app: String::new(),
                        app_path: String::new(),
                        app_dir: String::new(),
                    });
                    continue;
                }
            };
            for (id, m) in targets {
                let filename = match template {
                    "publish-demo-common.yml" => format!("publish-{id}.yml"),
                    "build-signed-demo-apk.yml" => format!("build-signed-{id}-apk.yml"),
                    "screenshots.yml" => format!("screenshots-{id}.yml"),
                    "verify-release-tooling.yml" => format!("verify-release-tooling-{id}.yml"),
                    _ => format!("reconcile-failed-jitpack-{id}.yml"),
                };
                plans.push(RenderTarget {
                    name: filename,
                    template_name: template,
                    module: id.clone(),
                    module_path: m.path.trim_end_matches(':').into(),
                    module_dir: m.directory.clone(),
                    app: id.clone(),
                    app_path: m.path.trim_end_matches(':').into(),
                    app_dir: m.directory.clone(),
                });
            }
        }
    } else {
        for &(name, _) in TEMPLATES {
            plans.push(RenderTarget {
                name: name.into(),
                template_name: name,
                module: legacy_module.clone(),
                module_path: format!(":{legacy_module}"),
                module_dir: legacy_module.clone(),
                app: legacy_app.clone(),
                app_path: format!(":{legacy_app}"),
                app_dir: legacy_app.clone(),
            });
        }
    }
    let mut filenames = std::collections::BTreeSet::new();
    for target in &plans {
        let name = &target.name;
        ensure!(filenames.insert(name.clone()), "Generated workflow filename collision: {name}; choose a different module ID in android_modules");
    }
    let module_options = if let Some(modules) = &config.android_modules {
        modules
            .iter()
            .filter(|(_, m)| m.kind == crate::discovery::Kind::Library && m.selected)
            .map(|(id, _)| serde_json::to_string(id).unwrap())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        legacy_module.clone()
    };
    let mut release_workflow_names = vec![format!("SUB - Finalize {title} release")];
    if let Some(modules) = &config.android_modules {
        release_workflow_names.extend(
            modules
                .iter()
                .filter(|(_, m)| m.selected && m.kind == crate::discovery::Kind::Library)
                .map(|(id, _)| format!("Publish {id}")),
        );
    } else {
        release_workflow_names.push("Publish".into());
    }
    let release_workflow_names = serde_json::to_string(&release_workflow_names)?;
    let selected_tests = config.android_modules.as_ref().map(|modules| {
        modules
            .values()
            .filter(|m| m.selected)
            .map(|m| format!("{}:testDebugUnitTest", m.path.trim_end_matches(':')))
            .collect::<Vec<_>>()
            .join(" ")
    });
    plans.into_iter().map(|RenderTarget { name, template_name, module, module_path, module_dir, app, app_path, app_dir }| {
        let screenshot_assertion = if config.android_modules.is_some() { "test -f screenshot-manifest/images.sha256".to_string() } else { format!("test \"$(wc -l < screenshot-manifest/images.sha256)\" -eq {screenshot_count}") };
        let module_dir_shell = shell_path(&module_dir);
        let app_dir_shell = shell_path(&app_dir);
        let template = TEMPLATES.iter().find(|(n,_)| *n == template_name).unwrap().1;
        let app_checks = if config.android_modules.is_none() { format!("{app_path}:assembleDebug {app_path}:lintDebug") } else { String::new() };
        let concurrency = if config.android_modules.is_some() { "android-repository-release".to_owned() } else { format!("{legacy_module}-release") };
        let rendered = placeholders.replace_all(template, |captures: &Captures<'_>| match &captures[1] {
            "tooling_ref" => config.tooling_ref.as_str(),
            "workflows.module" => module.as_str(),
            "workflows.module_path" => module_path.as_str(),
            "workflows.module_directory" => module_dir.as_str(),
            "workflows.module_directory_shell" => module_dir_shell.as_str(),
            "workflows.module_options" => module_options.as_str(),
            "workflows.release_workflow_names" => release_workflow_names.as_str(),
            "workflows.concurrency" => concurrency.as_str(),
            "workflows.app_module" => app.as_str(),
            "workflows.app_path" => app_path.as_str(),
            "workflows.app_directory" => app_dir.as_str(),
            "workflows.app_directory_shell" => app_dir_shell.as_str(),
            "workflows.app_checks" => app_checks.as_str(),
            "workflows.test_tasks" => selected_tests.as_deref().unwrap_or("test"),
            "workflows.title" => title.as_str(),
            "workflows.screenshot_count" => screenshot_count.as_str(),
            "workflows.screenshot_assertion" => screenshot_assertion.as_str(),
            "workflows.java_version" => java_version.as_str(),
            "workflows.documentation_command" => documentation_command.as_str(),
            "workflows.documentation_runtime" => docs_runtime.as_str(),
            "workflows.release_documentation_runtime" => release_runtime.as_str(),
            "workflows.screenshot_check_command" => screenshot_check_command.as_str(),
            "workflows.release_validation_command" => release_validation_command.as_str(),
            _ => "<<@ unknown @>>",
        }).into_owned();
        ensure!(!rendered.contains("<<@"), "Unresolved placeholder in {name}");
        let mut rendered = crate::publishing::render_workflow(root, template_name, &rendered)?;
        if config.android_modules.is_some() {
            if template_name == "publish-demo-common.yml" { rendered = rendered.replacen("name: Publish\n", &format!("name: Publish {module}\n"),1); }
            if template_name == "build-signed-demo-apk.yml" { rendered = rendered.replacen("name: Build signed APK", &format!("name: Build signed {app} APK"),1); }
        }
        let _: serde_yaml_ng::Value = serde_yaml_ng::from_str(&rendered).with_context(|| format!("Rendered workflow {name} is invalid YAML"))?;
        Ok((name, rendered))
    }).collect()
}

fn documentation_runtime(root: &Path, release: bool) -> String {
    let condition = "        if: steps.release.outputs.skip != 'true'\n";
    if root.join("sites/zola.toml").is_file() {
        let step = include_str!("../workflow-steps/install-zola.yml").trim_end();
        if release {
            step.replacen(
                "        shell: bash",
                &format!("{condition}        shell: bash"),
                1,
            )
        } else {
            step.to_owned()
        }
    } else {
        format!("      - uses: actions/setup-node@v7\n{}        with:\n          node-version: '24'\n          package-manager-cache: false", if release { condition } else { "" })
    }
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
        let drift = compare_and_apply(root, &output, &manifest, mode)?;
        return Ok(update_inventory(root, mode)? || drift);
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
    let drift = compare_and_apply(root, &output, &manifest, mode)?;
    Ok(update_inventory(root, mode)? || drift)
}

fn update_inventory(root: &Path, mode: Mode) -> Result<bool> {
    let Some(modules) = crate::discovery::load(root)? else {
        return Ok(false);
    };
    let path = root.join(".project/setup/modules.properties");
    let expected = crate::discovery::inventory(root, &modules)?;
    let previous = read_optional(&path)?;
    let drift = previous.as_deref() != Some(expected.as_str());
    if drift {
        setup_message!("modules.properties: update selected module inventory");
        if mode != Mode::Check && !(mode == Mode::OnlyMissing && previous.is_some()) {
            replace(&path, &expected)?;
        }
    }
    Ok(drift)
}
fn shell_path(value: &str) -> String {
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
        && !value.is_empty()
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn compare_and_apply(root: &Path, output: &Path, manifest: &Path, mode: Mode) -> Result<bool> {
    let mut hashes: BTreeMap<String, String> = match read_optional(manifest)? {
        Some(text) => serde_json::from_str(&text).context("Invalid workflow hash manifest")?,
        None => BTreeMap::new(),
    };
    let mut changes: Vec<(String, PathBuf, String, bool)> = Vec::new();
    let rendered = render(root)?;
    let desired: std::collections::BTreeSet<_> =
        rendered.iter().map(|(name, _)| name.clone()).collect();
    let mut obsolete = Vec::new();
    for (name, old_hash) in &hashes {
        ensure!(
            Path::new(name).file_name().and_then(|v| v.to_str()) == Some(name.as_str()),
            "Invalid workflow manifest filename"
        );
        if !desired.contains(name) {
            let path = output.join(name);
            if let Some(text) = read_optional(&path)? {
                ensure!(hash(&text) == *old_hash, "Obsolete workflow {name} has local changes; archive or remove it explicitly before regenerating");
                setup_message!("{name}: remove obsolete managed workflow");
                obsolete.push((name.clone(), path));
            }
        }
    }
    for (name, content) in rendered {
        let path = output.join(&name);
        let previous = read_optional(&path)?;
        if previous.as_deref() == Some(content.as_str())
            || (mode == Mode::OnlyMissing && previous.is_some())
        {
            continue;
        }
        let kind = match &previous {
            Some(text) if hashes.get(&name) != Some(&hash(text)) => "local/untracked changes",
            _ => "template update",
        };
        setup_message!("{name}: {kind}");
        setup_message!(
            "{}",
            TextDiff::from_lines(previous.as_deref().unwrap_or(""), &content)
                .unified_diff()
                .header(&name, &format!("{name} (generated)"))
        );
        changes.push((name, path, content, previous.is_some()));
    }
    if mode == Mode::Check {
        return Ok(!changes.is_empty() || !obsolete.is_empty());
    }
    if mode == Mode::Generate && (changes.iter().any(|entry| entry.3) || !obsolete.is_empty()) {
        bail!("Existing workflows differ. Review the diff and pass --overwrite to replace them.");
    }

    fs::create_dir_all(output)?;
    for (name, path, content, _) in &changes {
        replace(path, content)?;
        hashes.insert(name.to_owned(), hash(content));
    }
    if mode == Mode::Overwrite {
        for (name, path) in &obsolete {
            fs::remove_file(path)?;
            hashes.remove(name);
        }
        hashes.retain(|name, _| desired.contains(name));
    }
    if !changes.is_empty() || !obsolete.is_empty() || !manifest.exists() {
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
