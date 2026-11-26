use crate::{publishing, signing, vault, workflows};
use anyhow::{bail, ensure, Context, Result};
use clap::Args;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Config {
    repository: String,
    key_alias: Option<String>,
}

const OPERATIONS: [(&str, &str, &str); 5] = [
    (
        "key",
        "APK signing key",
        "Create or verify a local APK signing keystore, save its credentials in the encrypted vault, and write the public Android association file for the app.",
    ),
    (
        "local",
        "Local Gradle signing",
        "Write the saved keystore path and signing passwords to ignored local.properties for local builds. Requires an APK signing key and asks before writing plaintext credentials.",
    ),
    (
        "github",
        "APK signing environment",
        "Configure the GitHub APK signing environment and upload missing signing secrets from the local keystore. Existing secrets are preserved; the repository's branch policy is checked.",
    ),
    (
        "publishing",
        "Publishing environments",
        "Review shared library publishing credentials, create or update the required GitHub environments, and set GitHub Pages to deploy from Actions.",
    ),
    (
        "workflows",
        "GitHub workflows",
        "Generate missing GitHub Actions workflows for the selected module or repository, then review proposed changes before replacing existing files.",
    ),
];

pub(crate) fn signing_config(root: &Path) -> Result<vault::SetupConfig> {
    let config: Config = serde_yaml_ng::from_str(&fs::read_to_string(
        root.join(".project/setup/config.yaml"),
    )?)?;
    vault::SetupConfig::new(
        config.repository,
        "demo-apk-release".into(),
        "main".into(),
        None,
        None,
        Some(config.key_alias.unwrap_or_else(|| "demo".into())),
    )
}

#[derive(Args)]
pub(crate) struct SigningOptions {
    #[arg(long)]
    repo: Option<String>,
    #[arg(long, default_value = "demo-apk-release")]
    environment: String,
    #[arg(long, default_value = "main")]
    branch: String,
    #[arg(long)]
    state_dir: Option<PathBuf>,
    #[arg(long)]
    vault_path: Option<PathBuf>,
    #[arg(long)]
    key_alias: Option<String>,
}

pub(crate) fn sign(root: &Path, options: SigningOptions) -> Result<()> {
    vault::interactive()?;
    let (repository, key_alias) = match options.repo {
        Some(repository) => (repository, options.key_alias),
        None => {
            let config = signing_config(root)?;
            (config.repository, options.key_alias.or(config.key_alias))
        }
    };
    let config = vault::SetupConfig::new(
        repository,
        options.environment,
        options.branch,
        options.state_dir,
        options.vault_path,
        key_alias,
    )?;
    loop {
        match crate::ui::menu(&OPERATIONS[..3], !config.state_path().exists())? {
            crate::ui::Menu::Setup(selected) => {
                let operations = selected
                    .iter()
                    .map(|&index| OPERATIONS[index].0)
                    .collect::<Vec<_>>();
                let signing = ["key", "github"]
                    .into_iter()
                    .filter(|step| operations.contains(step))
                    .collect::<Vec<_>>();
                if (!signing.is_empty() || operations.contains(&"local"))
                    && !sign_with_config(root, &config, &signing, operations.contains(&"local"))?
                {
                    return Ok(());
                }
                return Ok(());
            }
            crate::ui::Menu::ScopedSetup { .. } => {
                bail!("Module-scoped setup is only available in the repository wizard.");
            }
            crate::ui::Menu::ViewVault => {
                vault::view(&config)?;
            }
            crate::ui::Menu::RemakeVault => {
                vault::remake(&config)?;
            }
            crate::ui::Menu::Exit => return Ok(()),
        }
    }
}

fn sign_with_config(
    root: &Path,
    config: &vault::SetupConfig,
    operations: &[&str],
    local_gradle: bool,
) -> Result<bool> {
    let _lock = vault::lock(config, true)?;
    let mut vault = vault::unlock_interactive(config, false)?;
    if !operations.is_empty() && !signing::run(root, config, &mut vault, operations)? {
        return Ok(false);
    }
    if local_gradle && !signing::configure_local_gradle_signing(root, &vault)? {
        return Ok(false);
    }
    Ok(true)
}

fn update_workflows(root: &Path, module_id: Option<&str>) -> Result<()> {
    let update = |mode| match module_id {
        Some(module_id) => workflows::update_module(root, mode, module_id),
        None => workflows::update(root, mode),
    };
    update(workflows::Mode::OnlyMissing)?;
    if update(workflows::Mode::Check)? {
        if !vault::confirm("Replace the workflows shown above?")? {
            return Ok(());
        }
        update(workflows::Mode::Overwrite)?;
    }
    if root.join(".project/setup/repositories.yml").try_exists()? {
        match module_id {
            Some(module_id) if crate::discovery::module(root, module_id)?.kind.is_library() => {
                publishing::generate_for_module(root, false, module_id)?;
            }
            Some(_) => {}
            None => publishing::generate(root, false)?,
        }
    }
    Ok(())
}

fn available_operations(
    has_apps: bool,
    has_libraries: bool,
) -> Vec<(&'static str, &'static str, &'static str)> {
    OPERATIONS
        .iter()
        .copied()
        .filter(|(id, _, _)| match *id {
            "key" | "local" | "github" => has_apps,
            "publishing" => has_libraries,
            _ => true,
        })
        .collect()
}

pub(crate) fn run(root: &Path, vault_created_at_handoff: bool) -> Result<()> {
    vault::interactive()?;
    crate::ui::phase("modules");
    let Some(modules) = crate::discovery::select(root)? else {
        return Ok(());
    };
    let has_apps = modules
        .values()
        .any(|m| m.selected && m.kind == crate::discovery::Kind::Application);
    let has_libraries = modules.values().any(|m| m.selected && m.kind.is_library());
    let available = available_operations(has_apps, has_libraries);
    let config = signing_config(root)?;
    let first_run = (vault_created_at_handoff || !config.state_path().exists())
        && !root
            .join(".project/setup/workflow-generation.json")
            .exists();
    if crate::ui::browser_active() && !vault::regular_file_exists(&config.state_path())? {
        crate::ui::phase("vault");
        let _lock = vault::lock(&config, true)?;
        // The browser creates its encrypted local vault before offering setup
        // actions. Later operations unlock it only when credentials are needed.
        let _vault = vault::unlock_interactive(&config, false)?;
    }
    crate::ui::phase("actions");
    loop {
        let menu = crate::ui::menu(&available, first_run)?;
        let (selected, module_id) = match menu {
            crate::ui::Menu::Setup(selected) => (selected, None),
            crate::ui::Menu::ScopedSetup { module_id, selected } => {
                ensure!(crate::ui::browser_active(), "Module-scoped setup requires the browser wizard.");
                let module = modules.get(&module_id).context("Unknown setup module")?;
                ensure!(module.selected, "The selected module is excluded from setup.");
                ensure!(!selected.is_empty(), "Choose at least one setup action.");
                for index in &selected {
                    let (id, _, _) = available.get(*index).context("Unknown setup operation")?;
                    ensure!(match *id {
                        "key" | "local" | "github" => module.kind == crate::discovery::Kind::Application,
                        "publishing" => module.kind.is_library(),
                        "workflows" => true,
                        _ => false,
                    }, "The chosen operation does not apply to this module.");
                }
                (selected, Some(module_id))
            }
            crate::ui::Menu::ViewVault => {
                vault::view(&config)?;
                continue;
            }
            crate::ui::Menu::RemakeVault => {
                vault::remake(&config)?;
                continue;
            }
            crate::ui::Menu::Exit => return Ok(()),
        };
        let operations = selected.iter().map(|&i| available[i].0).collect::<Vec<_>>();
        if operations.contains(&"workflows") {
            crate::ui::phase("workflows");
            update_workflows(root, module_id.as_deref())?;
        }
        let signing = ["key", "github"]
            .into_iter()
            .filter(|step| operations.contains(step))
            .collect::<Vec<_>>();
        if !signing.is_empty() || operations.contains(&"local") {
            crate::ui::phase("signing");
        }
        if (!signing.is_empty() || operations.contains(&"local"))
            && !sign_with_config(root, &config, &signing, operations.contains(&"local"))?
        {
            return Ok(());
        }
        if operations.contains(&"publishing") {
            crate::ui::phase("publishing");
            if let Some(module_id) = &module_id {
                setup_message!("Publishing environments and GitHub Pages are shared across libraries; configuring {module_id} uses the repository's shared credentials.");
            }
            publishing::setup(root, None, None)?;
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_kinds_expose_stable_action_ids_in_terminal_order() {
        let ids = |apps, libraries| {
            available_operations(apps, libraries)
                .into_iter()
                .map(|(id, _, _)| id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(true, false), ["key", "local", "github", "workflows"]);
        assert_eq!(ids(false, true), ["publishing", "workflows"]);
        assert_eq!(
            ids(true, true),
            ["key", "local", "github", "publishing", "workflows"]
        );
    }
}
