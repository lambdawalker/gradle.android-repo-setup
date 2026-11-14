//! Bundled Astro renderer. Only the current trusted checkout can override presentation.
use super::{content, Config};
use anyhow::{ensure, Context, Result};
use std::{
    path::Path,
    process::{Command, Output},
};

fn npm(root: &Path, args: &[&str], destination: Option<&Path>) -> Result<Output> {
    let command = |program: &std::ffi::OsStr| {
        let mut command = Command::new(program);
        command
            .current_dir(root)
            .args(args)
            .env("ASTRO_TELEMETRY_DISABLED", "1");
        if let Some(destination) = destination {
            command.env("ANDROID_REPO_DOCS_OUTPUT", destination);
        }
        command
    };
    let output = command("npm".as_ref()).output();
    #[cfg(windows)]
    let output = match output {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let base = std::env::current_dir().unwrap_or_else(|_| root.to_path_buf());
            let shim = std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| {
                        if dir.is_absolute() {
                            dir.join("npm.cmd")
                        } else {
                            base.join(dir).join("npm.cmd")
                        }
                    })
                    .find(|path| path.is_file())
            });
            match shim {
                Some(path) => command(path.as_os_str()).output(),
                None => Err(error),
            }
        }
        result => result,
    };
    output.context("npm is required for the shared Astro documentation site. Install Node.js 24 with npm, add both to PATH, then rerun build_docs.")
}

fn dependencies() -> Result<()> {
    let output = Command::new("node").arg("--version").output()
        .context("Node.js 24 is required for the shared Astro documentation site. Install Node.js 24 with npm and add both to PATH, then rerun build_docs.")?;
    let version = String::from_utf8_lossy(&output.stdout);
    let mut numbers = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(str::parse::<u32>);
    let major = numbers.next().transpose()?.unwrap_or(0);
    let minor = numbers.next().transpose()?.unwrap_or(0);
    ensure!(output.status.success() && (major > 22 || (major == 22 && minor >= 12)),
        "Shared Astro documentation requires Node.js 22.12 or newer; found {}. Install Node.js 24 with npm, then rerun build_docs.", version.trim());
    let output = npm(Path::new("."), &["--version"], None)?;
    ensure!(
        output.status.success(),
        "Could not run npm --version: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn bundle(stage: &Path) -> Result<()> {
    // Explicit compile-time embedding keeps cargo-installed binaries independent
    // from both the tooling source checkout and historical repository contents.
    macro_rules! embed { ($($path:literal),+ $(,)?) => { $(content::write(stage, $path, include_bytes!(concat!("../astro-theme/", $path)))?;)+ }; }
    embed!(
        "package.json",
        "package-lock.json",
        "astro.config.mjs",
        "src/content.config.ts",
        "src/components/ScopeSidebar.astro",
        "src/components/ScopeBanner.astro",
        "src/components/ScopePageTitle.astro",
        "src/components/ScopeHead.astro",
        "src/components/ScopeSearch.astro",
        "src/components/ScopeMarkdownContent.astro",
        "src/components/ScopeLanguageSelect.astro",
        "public/favicon.svg",
        "src/styles/docs.css",
        "src/pages/index.astro",
        "src/pages/[language]/index.astro",
        "scripts/markdown.mjs",
        "scripts/navigation.mjs",
        "scripts/prepare.mjs"
    );
    Ok(())
}

pub(super) fn build(root: &Path, base: Option<&str>, check: bool) -> Result<()> {
    super::check(root)?;
    dependencies()?;
    let config = Config::load(root)?;
    let base_url = config
        .site
        .as_ref()
        .and_then(|site| site.get("base_url"))
        .and_then(serde_json::Value::as_str);
    let temporary = tempfile::tempdir()?;
    let stage = temporary.path();
    bundle(stage)?;
    let prepared = content::prepare(root, stage, &config, base, base_url)?;
    for (from, to) in [
        ("components", "src/components"),
        ("styles", "src/styles"),
        ("public", "public"),
    ] {
        content::copy_tree(&root.join("sites/astro").join(from), &stage.join(to))?;
    }
    content::copy_tree(&stage.join("static"), &stage.join("public"))?;
    let installed = npm(
        stage,
        &["ci", "--ignore-scripts", "--no-audit", "--no-fund"],
        None,
    )?;
    ensure!(
        installed.status.success(),
        "Installing bundled Astro dependencies failed:\n{}\n{}",
        String::from_utf8_lossy(&installed.stdout),
        String::from_utf8_lossy(&installed.stderr)
    );
    let sites = std::path::absolute(root.join("sites"))?;
    std::fs::create_dir_all(&sites)?;
    let output = tempfile::Builder::new()
        .prefix(".astro-build-")
        .tempdir_in(&sites)?;
    let generated = output.path().join("dist");
    std::fs::create_dir_all(&generated)?;
    let result = npm(stage, &["run", "build"], Some(&generated))?;
    ensure!(
        result.status.success(),
        "Astro build failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    ensure!(
        generated.join("index.html").is_file(),
        "Astro build did not generate index.html"
    );
    if check {
        content::validate_links(&generated, &prepared.prefix, &prepared.url)?;
    }
    let destination = sites.join("dist");
    let previous = output.path().join("previous");
    let had_previous = destination.exists();
    if had_previous {
        std::fs::rename(&destination, &previous)
            .context("Could not preserve previous documentation output")?;
    }
    if let Err(error) = std::fs::rename(&generated, &destination) {
        if had_previous {
            if let Err(rollback) = std::fs::rename(&previous, &destination) {
                let preserved = output.keep();
                anyhow::bail!("Could not replace documentation output: {error}; rollback failed: {rollback}. Previous output preserved at {}", preserved.join("previous").display());
            }
        }
        return Err(error)
            .context("Could not replace documentation output; previous build restored");
    }
    Ok(())
}
