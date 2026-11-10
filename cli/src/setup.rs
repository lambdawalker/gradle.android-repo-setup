use crate::{publishing, signing, vault, workflows};
use anyhow::Result;
use clap::Args;
use dialoguer::{MultiSelect, Select};
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

const OPERATIONS: [(&str, &str, &str); 4] = [
    (
        "key",
        "APK signing key",
        "Create or verify the local APK signing keystore.",
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
    let _lock = vault::lock(&config, true)?;
    let mut vault = vault::unlock_interactive(&config, false)?;
    loop {
        let pending = &vault.state.pending;
        let complete = &vault.state.complete;
        if !pending.is_empty() {
            println!("Incomplete steps: {}", pending.join(", "));
        }
        let needs = |step: &str| {
            if pending.is_empty() {
                !complete.iter().any(|entry| entry == step)
            } else {
                pending.iter().any(|entry| entry == step)
            }
        };
        let default = match (needs("key"), needs("github")) {
            (true, true) => "3",
            (true, false) => "1",
            (false, true) => "2",
            _ => "q",
        };
        println!("[1] Create or verify the signing keystore");
        println!("[2] Set up GitHub using an existing keystore");
        println!("[3] Both operations");
        let can_reset = !pending.is_empty() || !complete.is_empty();
        if can_reset {
            println!("[r] Reset local setup progress and saved answers");
        }
        println!("[q] Exit");
        let answer = vault::prompt("Choose signing setup operations", Some(default))?;
        let operations: &[&str] = match answer.as_str() {
            "1" => &["key"],
            "2" => &["github"],
            "3" => &["key", "github"],
            "q" => return Ok(()),
            "r" if can_reset => {
                if vault::confirm("Clear local setup progress and answers? The APK keystore file and GitHub environment secrets will remain.")? {
                    vault.state = vault::VaultState::default();
                    vault.save()?;
                }
                continue;
            }
            _ => continue,
        };
        signing::run(root, &config, &mut vault, operations)?;
        return Ok(());
    }
}

fn sign_with_config(root: &Path, config: &vault::SetupConfig, operations: &[&str]) -> Result<bool> {
    let _lock = vault::lock(config, true)?;
    let mut vault = vault::unlock_interactive(config, false)?;
    signing::run(root, config, &mut vault, operations)
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
    let config = signing_config(root)?;
    let first_run = !config.state_path().exists()
        && !root
            .join(".project/setup/workflow-generation.json")
            .exists();
    loop {
        match Select::new()
            .with_prompt("Android Repository Setup")
            .items(["Configure deployment", "View vault", "Remake vault", "Exit"])
            .default(0)
            .interact_opt()?
        {
            Some(0) => {}
            Some(1) => return vault::view(&config).map(|_| ()),
            Some(2) => return vault::remake(&config).map(|_| ()),
            _ => return Ok(()),
        }
        let labels = OPERATIONS.map(|(_, label, description)| format!("{label} — {description}"));
        let Some(selected) = MultiSelect::new()
            .with_prompt("Choose setup operations")
            .items(&labels)
            .defaults(&[first_run; OPERATIONS.len()])
            .interact_opt()?
        else {
            continue;
        };
        if selected.is_empty() {
            continue;
        }
        let operations = selected
            .iter()
            .map(|&i| OPERATIONS[i].0)
            .collect::<Vec<_>>();
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
        if operations.contains(&"publishing") {
            publishing::setup(root, None, None)?;
        }
        return Ok(());
    }
}
