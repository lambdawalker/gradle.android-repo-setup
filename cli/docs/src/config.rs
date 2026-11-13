use crate::{matches, read};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path};

/// Documentation metadata is independent of publication, signing and setup state.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub schema: u32,
    pub repository: String,
    pub title: String,
    #[serde(default)]
    pub modules: serde_yaml_ng::Mapping,
    pub tooling_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshots: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<Value>,
}

// Legacy callers may retain any setup fields; none are needed or interpreted here.
#[derive(Deserialize)]
struct Legacy {
    repository: String,
    title: String,
    #[serde(default)]
    modules: serde_yaml_ng::Mapping,
    tooling_ref: String,
}

impl Config {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(".project/docs/config.yaml");
        let config: Self = if path.exists() {
            serde_yaml_ng::from_str(&read(path.clone())?)
                .with_context(|| format!("Invalid documentation config {}", path.display()))?
        } else {
            let legacy: Legacy =
                serde_yaml_ng::from_str(&read(root.join(".project/setup/config.yaml"))?)?;
            Self {
                schema: 1,
                repository: legacy.repository,
                title: legacy.title,
                modules: legacy.modules,
                tooling_ref: legacy.tooling_ref,
                sources: None,
                media: None,
                screenshots: None,
                site: None,
            }
        };
        ensure!(
            config.schema == 1,
            "Unsupported documentation config schema; expected 1"
        );
        ensure!(
            matches(
                r"[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9_.-]+",
                &config.repository
            ) && !matches!(config.repository.split('/').next_back(), Some("." | "..")),
            "Repository must have owner/name form"
        );
        ensure!(
            matches(
                r"(?:v[0-9]+\.[0-9]+\.[0-9]+|[0-9a-f]{40})",
                &config.tooling_ref
            ),
            "tooling_ref must be a release tag or full commit SHA"
        );
        for (name, label) in &config.modules {
            ensure!(
                name.as_str()
                    .is_some_and(|name| matches(r"[A-Za-z0-9_-][A-Za-z0-9_.-]*", name))
                    && label.is_string(),
                "Invalid documentation module configuration"
            );
        }
        Ok(config)
    }
    pub fn modules(&self) -> impl Iterator<Item = (&str, &str)> {
        self.modules
            .iter()
            .map(|(name, label)| (name.as_str().unwrap(), label.as_str().unwrap()))
    }
}

pub fn init_with(
    root: &Path,
    repository: Option<&str>,
    title: Option<&str>,
    tooling_ref: Option<&str>,
) -> Result<()> {
    let path = root.join(".project/docs/config.yaml");
    if path.exists() {
        Config::load(root)?;
        return Ok(());
    }
    let legacy = root.join(".project/setup/config.yaml");
    let mut config = if legacy.exists() {
        let mut c = Config::load(root)?;
        c.tooling_ref = format!("v{}", env!("CARGO_PKG_VERSION"));
        c
    } else {
        let inferred = std::process::Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| {
                s.trim()
                    .trim_end_matches(".git")
                    .replace("git@github.com:", "")
                    .replace("https://github.com/", "")
            });
        let repository = repository
            .map(str::to_owned)
            .or(inferred)
            .context("Provide --repository owner/name or a GitHub git origin")?;
        Config {
            schema: 1,
            title: title
                .unwrap_or(repository.split('/').next_back().unwrap_or("Documentation"))
                .into(),
            repository,
            modules: Default::default(),
            tooling_ref: format!("v{}", env!("CARGO_PKG_VERSION")),
            sources: None,
            media: None,
            screenshots: None,
            site: None,
        }
    };
    if let Some(v) = repository {
        config.repository = v.into();
    }
    if let Some(v) = title {
        config.title = v.into();
    }
    if let Some(v) = tooling_ref {
        config.tooling_ref = v.into();
    }
    ensure!(
        matches(
            r"[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9_.-]+",
            &config.repository
        ),
        "Repository must have owner/name form"
    );
    ensure!(
        matches(
            r"(?:v[0-9]+\.[0-9]+\.[0-9]+|[0-9a-f]{40})",
            &config.tooling_ref
        ),
        "tooling_ref must be a release tag or full commit SHA"
    );
    fs::create_dir_all(path.parent().unwrap())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    use std::io::Write;
    file.write_all(serde_yaml_ng::to_string(&config)?.as_bytes())?;
    Ok(())
}
