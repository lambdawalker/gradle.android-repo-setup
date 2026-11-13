use crate::{publishing, signing, vault, workflows};
use anyhow::Result;
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
        "Create or verify the local APK signing keystore.",
    ),
    (
        "local",
        "Local Gradle signing",
        "Write the saved signing identity to ignored local.properties for local builds.",
    ),
    (
        "github",
        "APK signing environment",
        "Configure the GitHub APK signing environment and signing secrets.",
    ),
    (
        "publishing",
        "Publishing environments",
        "Review publishing credentials and configure GitHub Pages to deploy from Actions.",
    ),
    (
        "workflows",
        "GitHub workflows",
        "Generate missing workflows and review changes before replacing existing files.",
    ),
];

fn signing_config(root: &Path) -> Result<vault::SetupConfig> {
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
                if !signing.is_empty() && !sign_with_config(root, &config, &signing)? {
                    return Ok(());
                }
                if operations.contains(&"local") {
                    local_signing_with_config(root, &config)?;
                }
                return Ok(());
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

fn sign_with_config(root: &Path, config: &vault::SetupConfig, operations: &[&str]) -> Result<bool> {
    let _lock = vault::lock(config, true)?;
    let mut vault = vault::unlock_interactive(config, false)?;
    signing::run(root, config, &mut vault, operations)
}

fn local_signing_with_config(root: &Path, config: &vault::SetupConfig) -> Result<bool> {
    let _lock = vault::lock(config, true)?;
    let vault = vault::unlock_interactive(config, false)?;
    signing::configure_local_gradle_signing(root, &vault)
}

fn update_workflows(root: &Path) -> Result<()> {
    workflows::update(root, workflows::Mode::OnlyMissing)?;
    if workflows::update(root, workflows::Mode::Check)? {
        if !vault::confirm("Replace the workflows shown above?")? {
            return Ok(());
        }
        workflows::update(root, workflows::Mode::Overwrite)?;
    }
    if root.join(".project/setup/repositories.yml").try_exists()? {
        publishing::generate(root, false)?;
    }
    Ok(())
}

pub(crate) fn run(root: &Path) -> Result<()> {
    vault::interactive()?;
    let Some(modules) = crate::discovery::select(root)? else {
        return Ok(());
    };
    let has_apps = modules
        .values()
        .any(|m| m.selected && m.kind == crate::discovery::Kind::Application);
    let has_libraries = modules
        .values()
        .any(|m| m.selected && m.kind == crate::discovery::Kind::Library);
    let available = OPERATIONS
        .iter()
        .copied()
        .filter(|(id, _, _)| match *id {
            "key" | "local" | "github" => has_apps,
            "publishing" => has_libraries,
            _ => true,
        })
        .collect::<Vec<_>>();
    let config = signing_config(root)?;
    loop {
        let first_run = !config.state_path().exists()
            && !root
                .join(".project/setup/workflow-generation.json")
                .exists();
        let selected = match crate::ui::menu(&available, first_run)? {
            crate::ui::Menu::Setup(selected) => selected,
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
            update_workflows(root)?;
        }
        let signing = ["key", "github"]
            .into_iter()
            .filter(|step| operations.contains(step))
            .collect::<Vec<_>>();
        if !signing.is_empty() && !sign_with_config(root, &config, &signing)? {
            return Ok(());
        }
        if operations.contains(&"local") && !local_signing_with_config(root, &config)? {
            return Ok(());
        }
        if operations.contains(&"publishing") {
            publishing::setup(root, None, None)?;
        }
        return Ok(());
    }
}
