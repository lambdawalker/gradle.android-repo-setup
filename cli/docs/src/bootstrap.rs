//! Repository launchers for using the documentation executable without setup.
use anyhow::{ensure, Context, Result};
use std::{collections::BTreeMap, fs, io::Write, path::Path};

pub fn expected(root: &Path) -> Result<BTreeMap<&'static str, String>> {
    let text = fs::read_to_string(root.join(".project/docs/config.yaml"))?;
    let config: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text)?;
    let pin = config["tooling_ref"]
        .as_str()
        .context("Missing documentation tooling_ref")?;
    ensure!(
        regex::Regex::new(r"\A(?:v[0-9]+\.[0-9]+\.[0-9]+|[0-9a-f]{40})\z")?.is_match(pin),
        "Documentation tooling_ref must be a release tag or full lowercase commit SHA"
    );
    // Keep the dependency-free launchers and YAML parser on the same accepted
    // boundary: one ordinary top-level scalar line, with optional quotes/comments.
    let line_pattern = regex::Regex::new(
        r#"^(tooling_ref|'tooling_ref'|"tooling_ref")[ \t]*:[ \t]*([a-z0-9.]+|'[a-z0-9.]+'|"[a-z0-9.]+")([ \t]+#.*|[ \t]*)$"#,
    )?;
    let pins: Vec<_> = text
        .lines()
        .filter_map(|line| line_pattern.captures(line))
        .map(|capture| capture[2].trim_matches(['\'', '"']).to_owned())
        .collect();
    ensure!(pins.len() == 1 && pins[0] == pin,
        "Write tooling_ref on a single top-level line without indentation, using a plain or quoted key and pin (optional trailing comment), for example: tooling_ref: {pin} # docs tool. Block scalars, escapes, anchors and flow mappings are not supported by the launchers");
    let mut files = BTreeMap::from([
        (
            "docs.sh",
            include_str!("../bootstrap/docs.sh").replace("@PIN@", pin),
        ),
        ("docs.bat", include_str!("../bootstrap/docs.bat").to_owned()),
        (
            ".project/docs/bootstrap.ps1",
            include_str!("../bootstrap/bootstrap.ps1").replace("@PIN@", pin),
        ),
    ]);
    let mut ignore = fs::read_to_string(root.join(".gitignore"))
        .unwrap_or_default()
        .replace("\r\n", "\n");
    for pattern in [
        "/.project/docs/cache/",
        "/sites/.astro-app-*/",
        "/sites/.astro-build-*/",
    ] {
        if !ignore.lines().any(|line| line == pattern) {
            if !ignore.is_empty() && !ignore.ends_with('\n') {
                ignore.push('\n');
            }
            ignore.push_str(pattern);
            ignore.push('\n');
        }
    }
    files.insert(".gitignore", ignore);
    for text in files.values_mut() {
        *text = text.replace("\r\n", "\n");
    }
    Ok(files)
}

pub fn run(root: &Path, check: bool, overwrite: bool) -> Result<()> {
    let files = expected(root)?;
    for (name, expected) in &files {
        let mut path = root.to_owned();
        for part in Path::new(name).components() {
            path.push(part);
            if let Ok(meta) = fs::symlink_metadata(&path) {
                ensure!(
                    !meta.file_type().is_symlink(),
                    "Refusing symlink destination {}",
                    path.display()
                );
            }
        }
        if check || (path.exists() && !overwrite && *name != ".gitignore") {
            ensure!(fs::read_to_string(&path).ok().map(|s| s.replace("\r\n", "\n")).as_ref() == Some(expected), "Documentation bootstrap drift: {name}; run android-repo-docs bootstrap generate --overwrite");
        }
    }
    if check {
        return Ok(());
    }
    for (name, expected) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        staged.write_all(expected.as_bytes())?;
        #[cfg(unix)]
        if name == "docs.sh" {
            use std::os::unix::fs::PermissionsExt;
            staged
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o755))?;
        }
        staged.persist(path)?;
    }
    Ok(())
}
