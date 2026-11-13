//! Opt-in Zola renderer using shared immutable content staging.
use super::{
    content::{self, copy_tree, write},
    read, Config,
};
use anyhow::{ensure, Context, Result};
use std::{path::Path, process::Command};
const ZOLA_VERSION: &str = "0.23.6";
fn config_set(config: &str, section: &str, key: &str, value: &str) -> String {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut found = false;
    let mut section_seen = section.is_empty();
    for line in config.lines() {
        let trim = line.trim();
        if trim.starts_with('[') && trim.ends_with(']') {
            if current == section && !found {
                out.push(format!("{key} = {value}"));
                found = true;
            }
            current = trim[1..trim.len() - 1].to_owned();
            if current == section {
                section_seen = true;
            }
        }
        if current == section && trim.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
            out.push(format!("{key} = {value}"));
            found = true;
        } else {
            out.push(line.to_owned());
        }
    }
    if !found {
        if !section_seen {
            out.push(format!("[{section}]"));
        }
        out.push(format!("{key} = {value}"));
    }
    out.join("\n") + "\n"
}
pub(super) fn build(root: &Path, base: Option<&str>, check: bool) -> Result<()> {
    let version=Command::new("zola").arg("--version").output().context("Zola 0.23.6 is required. Install the pinned Zola binary and add it to PATH, then rerun build_docs.")?;
    ensure!(
        version.status.success()
            && String::from_utf8_lossy(&version.stdout).trim() == format!("zola {ZOLA_VERSION}"),
        "Zola {ZOLA_VERSION} is required; found {}. Install the pinned binary and add it to PATH.",
        String::from_utf8_lossy(&version.stdout).trim()
    );
    let config = Config::load(root)?;
    let stage = tempfile::tempdir()?;
    let stage = stage.path();
    let zola_config = read(root.join("sites/zola.toml"))?;
    let mut zola_config = config_set(&zola_config, "", "default_language", "\"en\"");
    zola_config = config_set(&zola_config, "", "build_search_index", "true");
    zola_config = config_set(&zola_config, "languages.es", "build_search_index", "true");
    zola_config = config_set(&zola_config, "search", "index_format", "\"fuse_json\"");
    zola_config = config_set(
        &zola_config,
        "",
        "title",
        &serde_json::to_string(&config.title)?,
    );
    zola_config = config_set(
        &zola_config,
        "extra",
        "repository",
        &serde_json::to_string(&config.repository)?,
    );
    write(stage, "zola.toml", zola_config)?;
    for (path, content) in [
        (
            "templates/base.html",
            include_str!("../site-theme/templates/base.html"),
        ),
        (
            "templates/index.html",
            include_str!("../site-theme/templates/index.html"),
        ),
        (
            "templates/page.html",
            include_str!("../site-theme/templates/page.html"),
        ),
        (
            "templates/section.html",
            include_str!("../site-theme/templates/section.html"),
        ),
        (
            "static/docs.css",
            include_str!("../site-theme/static/docs.css"),
        ),
        (
            "static/docs.js",
            include_str!("../site-theme/static/docs.js"),
        ),
    ] {
        write(stage, path, content)?;
    }
    write(
        stage,
        "static/favicon.svg",
        include_str!("../site-theme/static/favicon.svg"),
    )?;
    copy_tree(&root.join("sites/templates"), &stage.join("templates"))?;
    copy_tree(&root.join("sites/static"), &stage.join("static"))?;
    let prepared = content::prepare(root, stage, &config, base, None)?;
    let (url, prefix) = (prepared.url, prepared.prefix);
    let destination = root.join("sites/dist");
    let output = Command::new("zola")
        .current_dir(stage)
        .args([
            "--config",
            "zola.toml",
            "build",
            "--base-url",
            &url,
            "--output-dir",
        ])
        .arg(&destination)
        .arg("--force")
        .output()?;
    ensure!(
        output.status.success(),
        "Zola build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if check {
        content::validate_links(&destination, &prefix, &url)?;
    }
    Ok(())
}
