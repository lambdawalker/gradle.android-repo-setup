use crate::release::git;
use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path, process::Command};

#[derive(Deserialize)]
struct Config {
    modules: BTreeMap<String, String>,
}

pub(crate) fn build(root: &Path) -> Result<()> {
    let tag = std::env::var("VERSION").context("JitPack VERSION is required")?;
    let pattern = Regex::new(
        r"\A([A-Za-z0-9_-][A-Za-z0-9_.-]*)[/~]v((?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*)){2})-jitpack\z",
    )?;
    let fields = pattern
        .captures(&tag)
        .context("VERSION must be a canonical module/vX.Y.Z-jitpack tag")?;
    let module = &fields[1];
    let version = &fields[2];
    let config: Config = serde_yaml_ng::from_str(&fs::read_to_string(
        root.join(".project/setup/config.yaml"),
    )?)?;
    ensure!(
        config.modules.contains_key(module),
        "Unknown JitPack module"
    );
    let source = git(root, &["rev-parse", "HEAD"])?;
    ensure!(source.status.success(), "Could not resolve checkout");
    let source = std::str::from_utf8(&source.stdout)?.trim();
    let reference = format!("refs/tags/{module}/v{version}-jitpack^{{commit}}");
    let target = git(root, &["rev-parse", "--verify", &reference])?;
    ensure!(
        target.status.success() && std::str::from_utf8(&target.stdout)?.trim() == source,
        "Canonical tag does not identify checkout"
    );
    ensure!(
        std::env::var("GIT_COMMIT").as_deref().unwrap_or(source) == source,
        "JitPack GIT_COMMIT differs from checkout"
    );
    let wrapper = if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    };
    let selected = crate::discovery::module(root, module)?;
    ensure!(
        selected.selected && selected.kind.is_library(),
        "Module is not a selected library"
    );
    crate::assets::prepare(root, module, Some(source))?;
    let status = Command::new(root.canonicalize()?.join(wrapper))
        .current_dir(root)
        .args([
            "--no-daemon",
            &format!(
                "{}:publishToMavenLocal",
                selected.path.trim_end_matches(':')
            ),
            "-PjitpackBuild=true",
            &format!("-PreleaseModule={module}"),
            &format!("-PreleaseVersion={version}"),
            "-PjitpackScopedTag=true",
        ])
        .status()
        .context("Could not start the Gradle wrapper")?;
    ensure!(status.success(), "Gradle publication failed ({status})");
    Ok(())
}
