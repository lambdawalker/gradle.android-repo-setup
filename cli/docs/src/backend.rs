use crate::config::Config;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::{path::Path, process::Command};

const ACTIVE: &str = "ANDROID_REPO_DOCS_SITE_ACTIVE";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Site {
    #[serde(default)]
    build: Option<Vec<String>>,
    #[serde(default)]
    serve: Option<Vec<String>>,
}

pub fn run(
    root: &Path,
    config: &Config,
    operation: &str,
    base: Option<&str>,
    port: Option<u16>,
    check: bool,
) -> Result<bool> {
    let Some(value) = &config.site else {
        return Ok(false);
    };
    let site: Site = serde_json::from_value(value.clone())
        .context("Invalid site config; commands must be argv arrays")?;
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
