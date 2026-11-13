//! Generated, pinned launchers. Generation never executes downloaded code.
use anyhow::{ensure, Context, Result};
use std::{collections::BTreeMap, fs, path::Path};
pub(crate) fn expected(root: &Path) -> Result<BTreeMap<&'static str, String>> {
    let config: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&fs::read(root.join(crate::discovery::CONFIG))?)?;
    let pin = config["tooling_ref"]
        .as_str()
        .context("Missing tooling_ref")?;
    ensure!(
        regex::Regex::new(r"\A(?:v[0-9]+\.[0-9]+\.[0-9]+|[0-9a-f]{40})\z")?.is_match(pin),
        "tooling_ref must be a stable release tag or full lowercase commit SHA"
    );
    let mut files = BTreeMap::from([
        (
            "setup.sh",
            include_str!("../bootstrap/setup.sh").replace("@PIN@", pin),
        ),
        (
            "setup.bat",
            include_str!("../bootstrap/setup.bat").to_owned(),
        ),
        (
            ".project/setup/bootstrap.ps1",
            include_str!("../bootstrap/bootstrap.ps1").replace("@PIN@", pin),
        ),
        (
            ".github/workflows/verify-setup.yml",
            include_str!("../bootstrap/verify-setup.yml")
                .replace("@JAVA@", &java_versions(&config)),
        ),
    ]);
    if config["bootstrap"]["jitpack"].as_bool() == Some(true) {
        let existing = root.join("jitpack.yml");
        let mut jitpack: serde_yaml_ng::Value = if existing.exists() {
            serde_yaml_ng::from_slice(&fs::read(existing)?)?
        } else {
            serde_yaml_ng::from_str("jdk: [openjdk17]")?
        };
        jitpack["install"] = serde_yaml_ng::to_value(vec!["if ! command -v cargo >/dev/null 2>&1; then curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal; fi", "if [ -f \"$HOME/.cargo/env\" ]; then . \"$HOME/.cargo/env\"; fi; ANDROID_REPO_SETUP_SOURCE=1 sh ./setup.sh jitpack-build"])?;
        files.insert("jitpack.yml", serde_yaml_ng::to_string(&jitpack)?);
    }
    let ignore = root.join(".gitignore");
    let mut ignored = if ignore.exists() {
        fs::read_to_string(ignore)?
    } else {
        String::new()
    };
    if !ignored.lines().any(|line| line == "/.project/setup/cache/") {
        if !ignored.is_empty() && !ignored.ends_with('\n') {
            ignored.push('\n');
        }
        ignored.push_str("/.project/setup/cache/\n");
    }
    if root.join(".project/docs/config.yaml").is_file() {
        let workflow = files.get_mut(".github/workflows/verify-setup.yml").unwrap();
        *workflow = workflow.replace("          sh ./setup.sh module-release verify\n          sh ./setup.sh documentation-history verify", "          sh ./docs.sh check");
        *workflow = workflow.replace(
            "          fetch-depth: 0\n",
            "          fetch-depth: 0\n          lfs: true\n",
        );
        workflow.push_str("      - name: Verify standalone Windows documentation launcher\n        if: runner.os == 'Windows'\n        shell: cmd\n        run: docs.bat bootstrap check\n");
        for (name, contents) in android_repo_docs::bootstrap::expected(root)? {
            if name != ".gitignore" {
                files.insert(name, contents);
            }
        }
        if !ignored.lines().any(|line| line == "/.project/docs/cache/") {
            if !ignored.is_empty() && !ignored.ends_with('\n') {
                ignored.push('\n');
            }
            ignored.push_str("/.project/docs/cache/\n");
        }
    }
    files.insert(".gitignore", ignored);
    for content in files.values_mut() {
        *content = normalize_newlines(content);
    }
    Ok(files)
}
fn java_versions(config: &serde_yaml_ng::Value) -> String {
    let value = &config["workflows"]["java_version"];
    let version = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .unwrap_or_else(|| "17".into());
    // Values go into an indented YAML block, never executable shell text.
    if version != "17" && version.chars().all(|c| c.is_ascii_digit()) {
        format!("17\n            {version}")
    } else {
        "17".into()
    }
}
fn normalize_newlines(content: &str) -> String {
    content.replace("\r\n", "\n")
}
fn safe(root: &Path, relative: &str) -> Result<()> {
    let mut path = root.to_owned();
    for part in Path::new(relative).components() {
        path.push(part);
        if let Ok(meta) = fs::symlink_metadata(&path) {
            ensure!(
                !meta.file_type().is_symlink(),
                "Refusing symlink destination {}",
                path.display()
            );
        }
    }
    Ok(())
}
pub(crate) fn run(root: &Path, check: bool, overwrite: bool) -> Result<()> {
    let files = expected(root)?;
    for (name, content) in &files {
        safe(root, name)?;
        let path = root.join(name);
        if check {
            ensure!(
                fs::read_to_string(&path)
                    .ok()
                    .map(|text| normalize_newlines(&text))
                    .as_ref()
                    == Some(content),
                "Bootstrap drift: {name}; run bootstrap generate --overwrite"
            );
        } else if path.exists() && !overwrite && *name != ".gitignore" {
            ensure!(
                normalize_newlines(&fs::read_to_string(&path)?) == *content,
                "Refusing to overwrite {name}; use --overwrite"
            );
        }
    }
    if check {
        return Ok(());
    }
    for (name, content) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        use std::io::Write;
        staged.write_all(content.as_bytes())?;
        #[cfg(unix)]
        if name == "setup.sh" || name == "docs.sh" {
            use std::os::unix::fs::PermissionsExt;
            staged
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o755))?;
        }
        staged.persist(&path)?;
    }
    Ok(())
}
