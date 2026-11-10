use crate::github::{self, GitHubApi, Snapshot, Status};
use crate::{gpg, vault};
use anyhow::{bail, ensure, Context, Result};
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

const REGISTRY_PATH: &str = ".project/setup/repositories.yml";
const PROPERTIES_PATH: &str = ".project/setup/repositories.properties";
const WORKFLOWS: [&str; 2] = ["publish-demo-common.yml", "finalize-release.yml"];
const KEY: &str = "SIGNING_IN_MEMORY_KEY";
const KEY_PASSWORD: &str = "SIGNING_IN_MEMORY_KEY_PASSWORD";
const SECRET_LIMIT: usize = 48 * 1024;
const DELAYED_DOCS: &str = "delayed-docs";
const PROPERTIES_HEADER: &str =
    "# Generated from .project/setup/repositories.yml; run android-repo-setup publishing_config generate.";
const LEGACY_PROPERTIES_HEADER: &str =
    "# Generated from .project/setup/repositories.yml; run python scripts/publishing_config.py generate.";

type Registry = IndexMap<String, Destination>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    repositories: Registry,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Destination {
    pub(crate) environment: String,
    pub(crate) publisher: String,
}

pub(crate) fn load_registry(path: &Path) -> Result<Registry> {
    let input =
        fs::read_to_string(path).with_context(|| format!("Could not read {}", path.display()))?;
    // YAML Value rejects duplicate mapping keys before typed deserialization.
    let document: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&input).map_err(|_| anyhow::anyhow!("Invalid repository YAML"))?;
    let configuration: Configuration = serde_yaml_ng::from_value(document).map_err(|_| {
        anyhow::anyhow!(
            "Configuration must contain a repositories mapping of environments and publishers"
        )
    })?;
    let entries = configuration.repositories;
    ensure!(
        entries.contains_key("maven-central"),
        "Keep maven-central for existing release history and workflow defaults"
    );
    let mut seen = BTreeSet::new();
    for (id, entry) in &entries {
        ensure!(
            matches!(
                (id.as_str(), entry.publisher.as_str()),
                ("maven-central", "central") | ("jitpack", "jitpack")
            ),
            "Keep the maven-central ID for Central and the jitpack ID for JitPack"
        );
        ensure!(
            (1..=100).contains(&entry.environment.len())
                && entry
                    .environment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)),
            "Environment names must use letters, digits, underscores or hyphens"
        );
        ensure!(
            seen.insert(entry.environment.to_ascii_lowercase()),
            "Each repository must use its own environment"
        );
    }
    Ok(entries)
}

fn dropdown(content: &str, entries: &Registry) -> Result<String> {
    const BEGIN: &str = "# BEGIN GENERATED REPOSITORIES";
    const END: &str = "# END GENERATED REPOSITORIES";
    let mut result = String::new();
    let mut indent = None;
    let mut completed = false;
    let options = entries
        .keys()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join(", ");
    for line in content.split_inclusive('\n') {
        let text = line.trim_end_matches(['\r', '\n']);
        let inner = text.trim_start_matches(' ');
        let leading = &text[..text.len() - inner.len()];
        if inner == BEGIN {
            ensure!(
                !leading.is_empty() && indent.is_none() && !completed,
                "Expected exactly one repository dropdown marker pair per workflow"
            );
            result.push_str(line);
            let newline = if line.ends_with("\r\n") { "\r\n" } else { "\n" };
            result.push_str(&format!("{leading}options: [{options}]{newline}"));
            indent = Some(leading);
        } else if inner == END {
            ensure!(
                indent == Some(leading),
                "Repository dropdown markers must have matching indentation"
            );
            result.push_str(line);
            indent = None;
            completed = true;
        } else if indent.is_none() {
            result.push_str(line);
        }
    }
    ensure!(
        completed && indent.is_none(),
        "Expected exactly one repository dropdown marker pair per workflow"
    );
    Ok(result)
}

pub(crate) fn render_workflow(root: &Path, name: &str, content: &str) -> Result<String> {
    let registry = root.join(REGISTRY_PATH);
    if !WORKFLOWS.contains(&name) || !registry.try_exists()? {
        return Ok(content.to_owned());
    }
    dropdown(content, &load_registry(&registry)?)
}

pub(crate) fn generate(root: &Path, check: bool) -> Result<()> {
    let entries = load_registry(&root.join(REGISTRY_PATH))?;
    let mut properties = format!("{PROPERTIES_HEADER}\n");
    for (id, entry) in &entries {
        properties.push_str(&format!(
            "{id}.environment={}\n{id}.publisher={}\n",
            entry.environment, entry.publisher
        ));
    }
    let mut outputs = vec![(root.join(PROPERTIES_PATH), properties)];
    for name in WORKFLOWS {
        let path = root.join(".github/workflows").join(name);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Could not read {}", path.display()))?;
        outputs.push((path, dropdown(&content, &entries)?));
    }
    let mut drift = Vec::new();
    for (path, expected) in &outputs {
        let current = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(error).with_context(|| format!("Could not read {}", path.display()))
            }
        };
        if current == *expected {
            continue;
        }
        if check {
            if path == &root.join(PROPERTIES_PATH)
                && current.replacen(LEGACY_PROPERTIES_HEADER, PROPERTIES_HEADER, 1) == *expected
            {
                continue;
            }
            drift.push(
                path.strip_prefix(root)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
        } else {
            let mut file =
                tempfile::NamedTempFile::new_in(path.parent().context("Invalid output path")?)?;
            if let Ok(metadata) = fs::metadata(path) {
                file.as_file().set_permissions(metadata.permissions())?;
            }
            file.write_all(expected.as_bytes())?;
            file.persist(path)?;
        }
    }
    ensure!(
        drift.is_empty(),
        "Run android-repo-setup publishing_config generate; outdated files: {}",
        drift.join(", ")
    );
    Ok(())
}

fn destination(root: &Path, repository: &str) -> Result<Destination> {
    load_registry(&root.join(REGISTRY_PATH))?
        .shift_remove(repository)
        .context("Unknown repository; add it to .project/setup/repositories.yml first")
}

pub(crate) fn resolve(root: &Path, repository: &str) -> Result<()> {
    let entry = destination(root, repository)?;
    let output = format!(
        "environment={}\npublisher={}\n",
        entry.environment, entry.publisher
    );
    if let Some(path) = std::env::var_os("GITHUB_OUTPUT").filter(|path| !path.is_empty()) {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            .write_all(output.as_bytes())?;
    } else {
        std::io::stdout().write_all(output.as_bytes())?;
    }
    Ok(())
}

pub(crate) fn preflight(root: &Path, repository: &str) -> Result<()> {
    let entry = destination(root, repository)?;
    for setting in settings_for(&entry) {
        ensure!(
            !setting.required
                || std::env::var(setting.name).is_ok_and(|value| !value.trim().is_empty()),
            "Missing required setting: {}",
            setting.name
        );
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Secret,
    Variable,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Secret => "secret",
            Self::Variable => "variable",
        }
    }
}

struct Setting<'a> {
    environment: &'a str,
    name: &'static str,
    kind: Kind,
    required: bool,
    default: &'static str,
}

fn settings_for(entry: &Destination) -> Vec<Setting<'_>> {
    if entry.publisher == "jitpack" {
        return Vec::new();
    }
    [
        ("MAVEN_CENTRAL_USERNAME", true),
        ("MAVEN_CENTRAL_PASSWORD", true),
        (KEY, true),
        (KEY_PASSWORD, false),
    ]
    .into_iter()
    .map(|(name, required)| Setting {
        environment: &entry.environment,
        name,
        kind: Kind::Secret,
        required,
        default: "",
    })
    .collect()
}

impl Setting<'_> {
    fn existing(&self, snapshot: &Snapshot) -> bool {
        match self.kind {
            Kind::Secret => snapshot.secrets.contains(self.name),
            Kind::Variable => snapshot.variables.contains_key(self.name),
        }
    }

    fn choose(&self, input: String, existing: bool) -> Result<Option<String>> {
        if input.is_empty() {
            if existing || !self.required {
                return Ok(None);
            }
            ensure!(
                !self.default.is_empty(),
                "This setting is required because it does not exist yet."
            );
            return Ok(Some(self.default.to_owned()));
        }
        ensure!(
            !self.required || !input.trim().is_empty(),
            "This required setting cannot contain only whitespace."
        );
        self.validate(input).map(Some)
    }

    fn validate(&self, input: String) -> Result<String> {
        if self.name == KEY {
            return Ok(gpg::validate_signing_key(&input)?.to_owned());
        }
        ensure!(
            self.kind != Kind::Secret || input.len() <= SECRET_LIMIT,
            "The secret exceeds GitHub's 48 KiB limit."
        );
        Ok(input)
    }
}

fn resolve_value(
    setting: &Setting<'_>,
    saved: Option<&vault::SetupVault>,
    secrets: &Path,
) -> Option<(&'static str, String)> {
    let from_vault = saved
        .and_then(|vault| vault.state.publishing_values.get(setting.environment))
        .and_then(|values| values.get(setting.name))
        .cloned();
    for (source, value) in [
        ("encrypted vault", from_vault),
        ("local environment", std::env::var(setting.name).ok()),
    ] {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            if let Ok(value) = setting.validate(value) {
                return Some((source, value));
            }
        }
    }
    if setting.name == KEY {
        let data = vault::read_regular_file(&secrets.join("maven-signing-key.asc"), SECRET_LIMIT)
            .ok()??;
        let key = setting.validate(String::from_utf8(data).ok()?).ok()?;
        return Some(("local OpenPGP key file", key));
    }
    None
}

fn save_values(
    vault: Option<&mut vault::SetupVault>,
    values: &[(&Setting<'_>, &str)],
) -> Result<()> {
    if let Some(vault) = vault {
        for (setting, value) in values {
            vault
                .state
                .publishing_values
                .entry(setting.environment.to_owned())
                .or_default()
                .insert(setting.name.to_owned(), (*value).to_owned());
        }
        vault.save()?;
    }
    Ok(())
}

fn verify_state(
    api: &GitHubApi,
    environments: &IndexMap<&str, Snapshot>,
    settings: &[Setting<'_>],
    changes: &BTreeMap<usize, String>,
) -> Result<()> {
    for (environment, original) in environments {
        let relevant = changes
            .keys()
            .map(|index| &settings[*index])
            .filter(|setting| setting.environment == *environment)
            .collect::<Vec<_>>();
        if relevant.is_empty() && original.exists {
            continue;
        }
        let current = api.snapshot(environment)?;
        ensure!(
            original.exists == current.exists,
            "Environment {environment} changed during review. Rerun to inspect current state."
        );
        for setting in relevant {
            ensure!(
                setting.existing(original) == setting.existing(&current)
                    && (setting.kind != Kind::Variable
                        || original.variables.get(setting.name)
                            == current.variables.get(setting.name)),
                "{environment} / {} changed concurrently. Rerun to inspect current state.",
                setting.name
            );
        }
    }
    Ok(())
}

fn pages(api: &GitHubApi) -> Result<Option<Value>> {
    api.request("GET", &format!("{}/pages", api.root), None, true)
}

fn uses_actions(pages: &Option<Value>) -> bool {
    pages
        .as_ref()
        .is_some_and(|settings| settings["build_type"] == "workflow")
}

fn configure_pages(api: &GitHubApi, current: &Option<Value>) -> Result<()> {
    let latest = pages(api)?;
    if uses_actions(&latest) {
        return Ok(());
    }
    ensure!(
        &latest == current,
        "GitHub Pages changed during review. Rerun to inspect current settings."
    );
    api.request(
        if current.is_none() { "POST" } else { "PUT" },
        &format!("{}/pages", api.root),
        Some(&json!({"build_type": "workflow"})),
        false,
    )?;
    Ok(())
}

fn apply(
    api: &GitHubApi,
    environments: &IndexMap<&str, Snapshot>,
    settings: &[Setting<'_>],
    changes: &BTreeMap<usize, String>,
    keys: &BTreeSet<String>,
    page_settings: &Option<Value>,
) -> Result<()> {
    verify_state(api, environments, settings, changes)
        .context("Pre-apply environment check failed")?;
    for fingerprint in keys {
        gpg::publish_public_key(fingerprint)
            .context("Could not publish the selected public key")?;
        setup_message!("Completed: published public key {fingerprint}");
    }
    for (environment, snapshot) in environments {
        if snapshot.exists {
            continue;
        }
        let mut result = api.ensure_environment(
            environment,
            environment.eq_ignore_ascii_case(DELAYED_DOCS).then_some(15),
        );
        if environment.eq_ignore_ascii_case(DELAYED_DOCS) && status_is(&result, 422) {
            result = api.ensure_environment(environment, None);
            setup_message!("Wait timer unavailable. Review delayed-docs in GitHub Settings.");
        }
        let already_exists =
            result.with_context(|| format!("Failed or uncertain: {environment} / environment"))?;
        if already_exists
            && changes
                .keys()
                .any(|index| settings[*index].environment == *environment)
        {
            bail!(
                "Environment {environment} changed concurrently. Rerun to inspect current state."
            );
        }
        setup_message!(
            "Completed: {environment} / environment ({})",
            if already_exists {
                "already exists"
            } else {
                "created"
            }
        );
    }
    for (&index, value) in changes {
        let setting = &settings[index];
        let label = format!("{} / {}", setting.environment, setting.name);
        let result = match setting.kind {
            Kind::Variable => api.write_variable(
                setting.environment,
                setting.name,
                value,
                setting.existing(&environments[setting.environment]),
            ),
            Kind::Secret => api.write_secret(setting.environment, setting.name, value),
        };
        if setting.kind == Kind::Variable && status_is(&result, 409) {
            bail!("{label} was modified concurrently (HTTP 409 Conflict). Rerun to inspect current state.");
        }
        result.with_context(|| format!("Failed or uncertain: {label}"))?;
        setup_message!("Completed: {label}");
    }
    configure_pages(api, page_settings).context("Failed or uncertain: GitHub Pages")?;
    setup_message!("GitHub Pages uses GitHub Actions.");
    Ok(())
}

fn status_is<T>(result: &Result<T>, code: u16) -> bool {
    result
        .as_ref()
        .err()
        .and_then(|error| error.downcast_ref::<Status>())
        .is_some_and(|status| status.0 == code)
}

fn setup_lock(config: &vault::SetupConfig) -> Result<File> {
    let secrets = config.project_secrets_dir();
    vault::private_dir(&secrets)?;
    let identity = format!(
        "{:x}",
        Sha256::digest(config.repository.to_ascii_lowercase())
    );
    vault::lock_file(
        &secrets.join(format!("environments-{}.lock", &identity[..16])),
        "Another environment setup wizard is running for this repository. Close it before retrying.",
    )
}

fn select_vault(config: &vault::SetupConfig) -> Result<Option<vault::SetupVault>> {
    if !vault::confirm("Use the encrypted vault to reuse and save answers as you go?")? {
        return Ok(None);
    }
    loop {
        match vault::unlock_interactive(config, false) {
            Ok(vault) => return Ok(Some(vault)),
            Err(error) => {
                setup_message!("{error}");
                if !vault::confirm("Retry the vault?")? {
                    return Ok(None);
                }
            }
        }
    }
}

fn should_reuse(label: &str, source: &str) -> Result<bool> {
    if source == "encrypted vault" {
        vault::use_saved(label)
    } else {
        vault::confirm(&format!("Use the value for {label} from the {source}?"))
    }
}

fn connect(repository: &str, vault: Option<&vault::SetupVault>) -> Result<(GitHubApi, String)> {
    setup_message!("Connecting to {repository}. A fine-grained token needs repository Administration, Environments and Pages read/write permissions. A classic token needs repo scope and any required organization SSO authorization.");
    let from_vault = vault.and_then(|vault| vault.state.github_token.clone());
    let from_environment = ["GH_TOKEN", "GITHUB_TOKEN"].into_iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|token| !token.trim().is_empty())
    });
    let candidate = from_vault
        .filter(|token| !token.trim().is_empty())
        .map(|token| ("encrypted vault", token))
        .or_else(|| from_environment.map(|token| ("local environment", token)));
    let token = match candidate {
        Some((source, token)) if should_reuse("GitHub token", source)? => token,
        _ => vault::secret_prompt("GitHub access token")?,
    };
    let token = token.trim().to_owned();
    Ok((GitHubApi::new(repository, &token)?, token))
}

pub(crate) fn setup(root: &Path, repository: Option<&str>, config: Option<&Path>) -> Result<()> {
    #[derive(Deserialize)]
    struct Project {
        repository: String,
    }

    let project;
    let repository = match repository {
        Some(repository) => repository,
        None => {
            let path = root.join(".project/setup/config.yaml");
            project = serde_yaml_ng::from_str::<Project>(&fs::read_to_string(path)?)
                .map_err(|_| anyhow::anyhow!("Invalid project setup configuration"))?;
            &project.repository
        }
    };
    github::validate_repository(repository)?;
    let entries =
        load_registry(&config.map_or_else(|| root.join(REGISTRY_PATH), Path::to_path_buf))?;
    let config = vault::SetupConfig::new(
        repository.to_owned(),
        "demo-apk-release".into(),
        "main".into(),
        None,
        None,
        None,
    )?;
    vault::interactive()?;
    let _setup_lock = setup_lock(&config)?;
    let _vault_lock = vault::lock(&config, true)?;
    let mut vault = select_vault(&config)?;
    let (api, token) = connect(repository, vault.as_ref())?;
    let settings: Vec<_> = entries.values().flat_map(settings_for).collect();
    let mut environments = IndexMap::new();
    for environment in entries.values().map(|entry| entry.environment.as_str()) {
        environments.insert(environment, api.snapshot(environment)?);
    }
    if !environments
        .keys()
        .any(|name| name.eq_ignore_ascii_case(DELAYED_DOCS))
    {
        environments.insert(DELAYED_DOCS, api.snapshot(DELAYED_DOCS)?);
    }
    let page_settings = pages(&api)?;
    if let Some(vault) = &mut vault {
        if vault.state.github_token.as_deref() != Some(&token) {
            vault.state.github_token = Some(token);
            vault.save()?;
        }
    }

    let secrets = config.project_secrets_dir();
    let home = vault::expand_home(Path::new("~"))?;
    let mut changes = BTreeMap::new();
    let mut keys = BTreeSet::new();
    let mut index = 0;
    while index < settings.len() {
        let setting = &settings[index];
        let snapshot = &environments[setting.environment];
        let existing = setting.existing(snapshot);
        setup_message!(
            "\n{} / {} ({})",
            setting.environment,
            setting.name,
            setting.kind.label()
        );
        if existing {
            match setting.kind {
                Kind::Secret => setup_message!("Secret exists. Leave blank to keep it."),
                Kind::Variable => {
                    setup_message!("Current value: {:?}", snapshot.variables[setting.name])
                }
            }
        } else {
            setup_message!(
                "{}",
                if setting.required {
                    "Required."
                } else {
                    "Optional. Leave blank to skip."
                }
            );
        }
        if let Some((source, value)) = resolve_value(setting, vault.as_ref(), &secrets) {
            if should_reuse(setting.name, source)? {
                save_values(vault.as_mut(), &[(setting, &value)])?;
                changes.insert(index, value);
                index += 1;
                continue;
            }
        }
        if setting.name == KEY {
            if existing && vault::confirm("Keep the existing GitHub signing key?")? {
                index += 1;
                continue;
            }
            let mut prompt = |label: &str, masked: bool| {
                if masked {
                    vault::secret_prompt(label)
                } else {
                    vault::prompt(label, None)
                }
                .map(Some)
            };
            let Some(key) = gpg::pick_signing_key(&config.project_name(), &home, &mut prompt)?
            else {
                if vault::confirm("Cancel publishing setup?")? {
                    return Ok(());
                }
                continue;
            };
            let password = &settings[index + 1];
            save_values(
                vault.as_mut(),
                &[(setting, &key.armor), (password, &key.passphrase)],
            )?;
            keys.insert(key.fingerprint);
            changes.insert(index, key.armor);
            if !key.passphrase.is_empty() || password.existing(snapshot) {
                changes.insert(index + 1, key.passphrase);
            }
            index += 2;
            continue;
        }
        loop {
            let entered = match setting.kind {
                Kind::Secret => vault::secret_prompt("New value")?,
                Kind::Variable => vault::prompt("New value", None)?,
            };
            match setting.choose(entered, existing) {
                Ok(Some(value)) => {
                    save_values(vault.as_mut(), &[(setting, &value)])?;
                    changes.insert(index, value);
                    break;
                }
                Ok(None) => break,
                Err(error) => setup_message!("{error}"),
            }
        }
        index += 1;
    }

    setup_message!("\nReview changes for {repository}");
    for (environment, snapshot) in &environments {
        let action = if snapshot.exists {
            "KEEP environment and protection rules"
        } else if environment.eq_ignore_ascii_case(DELAYED_DOCS) {
            "CREATE environment with a 15-minute wait timer where supported"
        } else {
            "CREATE environment"
        };
        setup_message!("{action}  {environment}");
    }
    for (index, setting) in settings.iter().enumerate() {
        let existing = setting.existing(&environments[setting.environment]);
        let action = match (changes.contains_key(&index), existing) {
            (true, true) => "UPDATE",
            (true, false) => "CREATE",
            (false, true) => "KEEP",
            (false, false) => "LEAVE UNSET",
        };
        setup_message!(
            "{action}  {} / {} ({})",
            setting.environment,
            setting.name,
            setting.kind.label()
        );
        if setting.kind == Kind::Variable {
            if let Some(value) = changes.get(&index) {
                setup_message!("    New value: {value:?}");
            }
        }
    }
    for fingerprint in &keys {
        setup_message!("PUBLISH public key {fingerprint} to keyserver.ubuntu.com (including its public identity).");
    }
    if uses_actions(&page_settings) {
        setup_message!("KEEP GitHub Actions as the GitHub Pages source");
    } else {
        setup_message!("SET GitHub Actions as the GitHub Pages source");
    }
    if vault.is_some() {
        setup_message!("Accepted answers have been saved in the encrypted vault.");
    }
    setup_message!("GitHub writes are applied in sequence. Rerun after an interruption to inspect current settings.");
    if !vault::confirm("Apply these reviewed changes?")? {
        return Ok(());
    }

    apply(
        &api,
        &environments,
        &settings,
        &changes,
        &keys,
        &page_settings,
    )
    .context("Setup incomplete. Rerun to inspect current settings and resume.")?;
    setup_message!("Setup complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "repositories:\n  jitpack:\n    environment: jitpack\n    publisher: jitpack\n  maven-central:\n    environment: maven-central\n    publisher: central\n";
    const WORKFLOW: &str = "    # BEGIN GENERATED REPOSITORIES\n    options: [\"maven-central\"]\n    # END GENERATED REPOSITORIES\n";

    #[test]
    fn repository_generation_preserves_config_order_and_accepts_legacy_header() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".project/setup")).unwrap();
        fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
        fs::write(root.path().join(REGISTRY_PATH), CONFIG).unwrap();
        for name in WORKFLOWS {
            fs::write(root.path().join(".github/workflows").join(name), WORKFLOW).unwrap();
        }
        assert!(generate(root.path(), true).is_err());
        generate(root.path(), false).unwrap();
        let path = root.path().join(PROPERTIES_PATH);
        let properties = fs::read_to_string(&path).unwrap();
        assert!(properties.contains(
            "\njitpack.environment=jitpack\njitpack.publisher=jitpack\nmaven-central.environment="
        ));
        for name in WORKFLOWS {
            let output =
                fs::read_to_string(root.path().join(".github/workflows").join(name)).unwrap();
            assert_eq!(
                output,
                render_workflow(root.path(), name, WORKFLOW).unwrap()
            );
            assert!(output.contains("options: [\"jitpack\", \"maven-central\"]"));
        }
        fs::write(
            path,
            properties.replacen(PROPERTIES_HEADER, LEGACY_PROPERTIES_HEADER, 1),
        )
        .unwrap();
        generate(root.path(), true).unwrap();
    }

    #[test]
    fn registry_rejects_duplicate_keys_and_case_insensitive_environment_collision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("repositories.yml");
        fs::write(
            &path,
            CONFIG.replace(
                "publisher: jitpack",
                "publisher: jitpack\n    publisher: jitpack",
            ),
        )
        .unwrap();
        assert!(load_registry(&path).is_err());
        fs::write(
            &path,
            CONFIG.replace("environment: jitpack", "environment: MAVEN-CENTRAL"),
        )
        .unwrap();
        assert!(load_registry(&path).is_err());
    }
}
