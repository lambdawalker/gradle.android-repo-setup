use crate::config::Config;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::{path::Path, process::Command};

/// Documentation renderer selection shared by builds and generated CI workflows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SiteBackend {
    Astro,
    Zola,
    External,
    Legacy,
}

const ACTIVE: &str = "ANDROID_REPO_DOCS_SITE_ACTIVE";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Site {
    #[serde(default)]
    backend: Option<SiteBackend>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    build: Option<Vec<String>>,
    #[serde(default)]
    serve: Option<Vec<String>>,
    #[serde(default)]
    _pages: Option<serde_json::Value>,
    #[serde(default)]
    _translation_manifest: Option<serde_json::Value>,
    #[serde(default)]
    _translation_paths: Option<serde_json::Value>,
    #[serde(default)]
    _aliases: Option<serde_json::Value>,
}

fn settings(value: Option<&serde_json::Value>) -> Result<Option<Site>> {
    value.map(|value| {
        let site: Site = serde_json::from_value(value.clone())
            .context("Invalid site config; backend must be astro, zola, external or legacy, and commands must be argv arrays")?;
        let commands = site.build.is_some() || site.serve.is_some();
        ensure!(!commands || site.backend.is_none() || site.backend == Some(SiteBackend::External),
            "Built-in site.backend cannot be combined with site.build or site.serve; use backend: external for custom commands");
        ensure!(site.backend != Some(SiteBackend::External) || commands,
            "External site backend requires site.build or site.serve");
        if let Some(url) = &site.base_url {
            ensure!((url.starts_with("https://") || url.starts_with("http://")) && !url.contains(['\\', '\0', '\n', '\r', '?', '#']),
                "site.base_url must be an absolute HTTP(S) URL without query or fragment");
        }
        Ok(site)
    }).transpose()
}

pub(crate) fn validate(config: &Config) -> Result<()> {
    settings(config.site.as_ref())?;
    Ok(())
}

pub(crate) fn selected(root: &Path, config: &Config) -> Result<SiteBackend> {
    select(root, config.site.as_ref())
}

pub(crate) fn select(root: &Path, value: Option<&serde_json::Value>) -> Result<SiteBackend> {
    if let Some(site) = settings(value)? {
        if let Some(backend) = site.backend {
            return Ok(backend);
        }
        if site.build.is_some() || site.serve.is_some() {
            return Ok(SiteBackend::External);
        }
    }
    Ok(if root.join("sites/zola.toml").is_file() {
        SiteBackend::Zola
    } else {
        SiteBackend::Astro
    })
}

pub fn run(
    root: &Path,
    config: &Config,
    operation: &str,
    base: Option<&str>,
    port: Option<u16>,
    check: bool,
) -> Result<bool> {
    if selected(root, config)? != SiteBackend::External {
        return Ok(false);
    }
    let site =
        settings(config.site.as_ref())?.context("External site backend requires commands")?;
    let argv = if operation == "build" {
        site.build
    } else {
        site.serve
    };
    let argv =
        argv.with_context(|| format!("Configure site.{operation} for the external site backend"))?;
    ensure!(
        !argv.is_empty() && !argv[0].is_empty(),
        "site.{operation} must contain an executable"
    );
    ensure!(
        std::env::var_os(ACTIVE).is_none(),
        "Recursive documentation site command rejected"
    );
    let command = |program: &std::ffi::OsStr| {
        let mut command = Command::new(program);
        command
            .args(&argv[1..])
            .current_dir(root)
            .env(ACTIVE, "1")
            .env("ANDROID_REPO_DOCS_CHECK", if check { "1" } else { "0" });
        if let Some(base) = base {
            command.env("ANDROID_REPO_DOCS_BASE", base);
        }
        if let Some(port) = port {
            command.env("ANDROID_REPO_DOCS_PORT", port.to_string());
        }
        command
    };
    let status = command(argv[0].as_ref()).status();
    #[cfg(windows)]
    let status = match status {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match windows_command_shim(root, &argv[0]) {
                Some(shim) => command(shim.as_os_str()).status(),
                None => Err(error),
            }
        }
        result => result,
    };
    let status =
        status.with_context(|| format!("Could not execute site.{operation}: {}", argv[0]))?;
    ensure!(status.success(), "site.{operation} failed with {status}");
    Ok(true)
}

#[cfg(windows)]
fn windows_command_shim(root: &Path, program: &str) -> Option<std::path::PathBuf> {
    let program = Path::new(program);
    // Native executables keep their normal Windows resolution. Only a missing
    // bare command may fall back to a PATH shim (such as Node's npm.cmd).
    if program.components().count() != 1 || program.extension().is_some() {
        return None;
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for extension in ["cmd", "bat"] {
            let candidate = root
                .join(&directory)
                .join(program)
                .with_extension(extension);
            if candidate.is_file() {
                // Passing the path and argv directly lets Rust handle the batch
                // interpreter's quoting; never construct a shell command string.
                return Some(candidate);
            }
        }
    }
    None
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn built_in_backend_accepts_renderer_specific_site_metadata() -> Result<()> {
        let site = json!({
            "backend": "astro",
            "base_url": "https://example.com/docs/",
            "pages": {"README.md": "project"},
            "translation_manifest": "translations.json",
            "translation_paths": {"README.md": "docs/es/README.md"},
            "aliases": [{"source": "README.md", "html": "{language}/main/README.html"}]
        });
        assert_eq!(
            select(Path::new("."), Some(&site))?,
            SiteBackend::Astro
        );
        Ok(())
    }
}
