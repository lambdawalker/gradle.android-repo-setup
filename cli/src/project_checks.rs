//! Declarative project checks replacing consumer-owned validation scripts.
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    screenshots: BTreeMap<String, Screenshots>,
    #[serde(default)]
    junit: BTreeMap<String, Junit>,
    #[serde(default)]
    release_validation: Vec<Vec<String>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Screenshots {
    directory: String,
    scenarios: Vec<String>,
    #[serde(default)]
    metadata: BTreeMap<String, Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Junit {
    directory: String,
    required: Vec<[String; 2]>,
}
fn config(root: &Path) -> Result<Config> {
    Ok(serde_json::from_slice(&fs::read(
        root.join(".project/verification/checks.json"),
    )?)?)
}
fn path(root: &Path, value: &str) -> Result<PathBuf> {
    ensure!(
        !value.is_empty() && !value.contains('\\') && !value.contains(':'),
        "Invalid project path: {value}"
    );
    let relative = Path::new(value);
    ensure!(
        relative
            .components()
            .all(|c| matches!(c, Component::Normal(_))),
        "Invalid project path: {value}"
    );
    let mut result = root.to_path_buf();
    for c in relative.components() {
        result.push(c);
        if let Ok(metadata) = fs::symlink_metadata(&result) {
            ensure!(
                !metadata.file_type().is_symlink(),
                "Symlinked project path: {value}"
            );
        }
    }
    Ok(result)
}
fn files(root: &Path, extension: &str, output: &mut Vec<PathBuf>) -> Result<()> {
    for item in fs::read_dir(root).with_context(|| format!("Read {}", root.display()))? {
        let item = item?;
        let kind = item.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "Symlink in check inputs: {}",
            item.path().display()
        );
        if kind.is_dir() {
            files(&item.path(), extension, output)?;
        } else if kind.is_file() && item.path().extension().is_some_and(|v| v == extension) {
            output.push(item.path());
        }
    }
    output.sort();
    Ok(())
}
fn image_record(directory: &Path, scenarios: &[String]) -> Result<Value> {
    ensure!(
        !scenarios.is_empty() && scenarios.iter().collect::<BTreeSet<_>>().len() == scenarios.len(),
        "Empty or duplicate screenshot scenarios"
    );
    let mut images = Vec::new();
    files(directory, "png", &mut images)?;
    let mut names = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    let mut dimensions = BTreeMap::new();
    for scenario in scenarios {
        ensure!(!scenario.is_empty(), "Empty scenario name");
        let matches: Vec<_> = images
            .iter()
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(&format!("_{scenario}_"))
            })
            .collect();
        ensure!(
            matches.len() == 1,
            "Expected exactly one screenshot for {scenario}, found {}",
            matches.len()
        );
        names.insert(
            scenario.clone(),
            matches[0]
                .strip_prefix(directory)?
                .to_string_lossy()
                .replace('\\', "/"),
        );
    }
    ensure!(
        names.values().collect::<BTreeSet<_>>().len() == images.len()
            && names.len() == images.len(),
        "Unexpected or ambiguous screenshot files"
    );
    for image in images {
        let bytes = fs::read(&image)?;
        ensure!(
            bytes.len() >= 33 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" && &bytes[12..16] == b"IHDR",
            "Invalid PNG: {}",
            image.display()
        );
        let width = u32::from_be_bytes(bytes[16..20].try_into()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into()?);
        ensure!(width > 0 && height > 0, "Invalid PNG dimensions");
        let name = image
            .strip_prefix(directory)?
            .to_string_lossy()
            .replace('\\', "/");
        hashes.insert(name.clone(), format!("{:x}", Sha256::digest(&bytes)));
        dimensions.insert(name, json!({"width":width,"height":height}));
    }
    Ok(json!({"scenarios":names,"images":hashes,"dimensions":dimensions}))
}
pub(crate) fn screenshots(root: &Path, module: &str) -> Result<()> {
    let config = config(root)?;
    let spec = config
        .screenshots
        .get(module)
        .context("No screenshot contract for module")?;
    let directory = path(root, &spec.directory)?;
    let mut record = image_record(&directory, &spec.scenarios)?;
    for (key, value) in &spec.metadata {
        ensure!(
            ![
                "schema",
                "source",
                "module",
                "scenarios",
                "images",
                "dimensions"
            ]
            .contains(&key.as_str()),
            "Reserved screenshot metadata key: {key}"
        );
        record[key] = value.clone();
    }
    let source = crate::release::git(root, &["rev-parse", "HEAD"])?;
    ensure!(source.status.success(), "Cannot resolve source");
    record["schema"] = json!(1);
    record["source"] = json!(String::from_utf8(source.stdout)?.trim());
    record["module"] = json!(module);
    let output = path(root, &format!("{}/render-manifest.json", spec.directory))?;
    let mut tmp = tempfile::NamedTempFile::new_in(&directory)?;
    use std::io::Write;
    writeln!(tmp, "{}", serde_json::to_string_pretty(&record)?)?;
    tmp.persist(output)?;
    println!(
        "Validated {} screenshots for {module}",
        spec.scenarios.len()
    );
    Ok(())
}
fn junit_check(directory: &Path, required: &[[String; 2]]) -> Result<()> {
    let expected: BTreeSet<_> = required.iter().cloned().collect();
    ensure!(
        !expected.is_empty() && expected.len() == required.len(),
        "Empty or duplicate required tests"
    );
    let mut reports = Vec::new();
    files(directory, "xml", &mut reports)?;
    let mut passed = BTreeSet::new();
    for report in reports {
        if !report
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("TEST-")
        {
            continue;
        }
        let text = fs::read_to_string(report)?;
        let xml = roxmltree::Document::parse(&text)?;
        for case in xml.descendants().filter(|n| n.has_tag_name("testcase")) {
            let class = case.attribute("classname").unwrap_or("");
            let name = case.attribute("name").unwrap_or("");
            for expected in expected.iter().filter(|e| {
                (e[0] == class
                    || (!e[0].contains('.') && e[0] == class.rsplit('.').next().unwrap_or("")))
                    && e[1] == name
            }) {
                ensure!(
                    !case
                        .children()
                        .any(|n| ["failure", "error", "skipped"].contains(&n.tag_name().name())),
                    "Required test failed or skipped: {class}.{name}"
                );
                passed.insert(expected.clone());
            }
        }
    }
    ensure!(
        passed == expected,
        "Missing required tests: {:?}",
        expected.difference(&passed).collect::<Vec<_>>()
    );
    Ok(())
}
pub(crate) fn junit(root: &Path, suite: &str) -> Result<()> {
    let config = config(root)?;
    let spec = config
        .junit
        .get(suite)
        .context("No JUnit contract for suite")?;
    junit_check(&path(root, &spec.directory)?, &spec.required)?;
    println!(
        "Validated {} required tests for {suite}",
        spec.required.len()
    );
    Ok(())
}
/// Signal handlers only set an atomic flag. Child termination, waiting and Git
/// cleanup happen on the normal execution thread while handlers remain installed.
struct Cancellation {
    requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handlers: Vec<signal_hook::SigId>,
}
impl Cancellation {
    fn new() -> Result<Self> {
        let mut cancellation = Self {
            requested: Default::default(),
            handlers: Vec::new(),
        };
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            cancellation.handlers.push(signal_hook::flag::register(
                signal,
                cancellation.requested.clone(),
            )?);
        }
        Ok(cancellation)
    }
    fn check(&self) -> Result<()> {
        ensure!(
            !self.requested.load(std::sync::atomic::Ordering::SeqCst),
            "Release validation cancelled"
        );
        Ok(())
    }
    fn run(&self, command: &mut Command) -> Result<std::process::ExitStatus> {
        self.check()?;
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Keep all descendants in an isolated process group so cancelling
            // a shell/Gradle launcher also terminates its running subprocesses.
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        loop {
            if self.check().is_err() {
                #[cfg(unix)]
                {
                    // SAFETY: child.id() names the process group created above.
                    // kill performs no signal-handler IO; this is the normal thread.
                    unsafe {
                        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
                    }
                }
                #[cfg(windows)]
                {
                    let _ = Command::new("taskkill")
                        .args(["/PID", &child.id().to_string(), "/T", "/F"])
                        .output();
                }
                // Reap even if the child exited concurrently with cancellation.
                let _ = child.kill();
                child
                    .wait()
                    .context("Wait for cancelled validation command")?;
                self.check()?;
            }
            if let Some(status) = child.try_wait()? {
                self.check()?;
                return Ok(status);
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        for handler in self.handlers.drain(..) {
            signal_hook::low_level::unregister(handler);
        }
    }
}
/// Run committed argv steps in a disposable detached worktree, never in the release checkout.
pub(crate) fn release_validate(root: &Path, source: &str) -> Result<()> {
    ensure!(
        source.len() == 40 && source.bytes().all(|b| b.is_ascii_hexdigit()),
        "Expected full source commit SHA"
    );
    // Register before creating the worktree and retain handlers until cleanup
    // completes, including when a signal arrives during Git setup or teardown.
    let cancellation = Cancellation::new()?;
    let temp = tempfile::tempdir()?;
    let checkout = temp.path().join("source");
    let result = (|| -> Result<()> {
        cancellation.check()?;
        let output = crate::release::git(
            root,
            &[
                "worktree",
                "add",
                "--detach",
                checkout.to_str().context("Invalid temp path")?,
                source,
            ],
        )?;
        ensure!(output.status.success(), "Cannot create validation worktree");
        cancellation.check()?;
        let config = config(&checkout)?;
        ensure!(
            !config.release_validation.is_empty(),
            "No release validation steps configured"
        );
        for args in config.release_validation {
            ensure!(
                !args.is_empty() && !args[0].is_empty(),
                "Empty validation command"
            );
            let status = cancellation
                .run(
                    Command::new(&args[0])
                        .args(&args[1..])
                        .current_dir(&checkout)
                        .env("SOURCE_SHA", source),
                )
                .with_context(|| format!("Run validation command {}", args[0]))?;
            ensure!(
                status.success(),
                "Release validation command failed: {}",
                args[0]
            );
        }
        cancellation.check()
    })();
    // Always remove the Git registration before TempDir removes its directory.
    // A failed `worktree add` can still have created the checkout, so this also
    // runs on setup failure when the path exists.
    if checkout.exists() {
        let cleanup = crate::release::git(
            root,
            &["worktree", "remove", "--force", checkout.to_str().unwrap()],
        )
        .context("Clean validation worktree")?;
        ensure!(
            cleanup.status.success(),
            "Could not clean validation worktree: {}",
            String::from_utf8_lossy(&cleanup.stderr)
        );
    }
    result?;
    cancellation.check()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn screenshot_checks_names_bytes_and_extras() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let p = tmp.path().join("Fixture_front_hash.png");
        let mut bytes = vec![0; 33];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes[12..16].copy_from_slice(b"IHDR");
        bytes[19] = 1;
        bytes[23] = 2;
        fs::write(&p, &bytes)?;
        let names = vec!["front".into()];
        let result = image_record(tmp.path(), &names)?;
        assert_eq!(result["dimensions"]["Fixture_front_hash.png"]["height"], 2);
        assert!(image_record(tmp.path(), &["missing".into()]).is_err());
        fs::write(tmp.path().join("extra.png"), &bytes)?;
        assert!(image_record(tmp.path(), &names).is_err());
        fs::remove_file(tmp.path().join("extra.png"))?;
        fs::write(p, b"invalid")?;
        assert!(image_record(tmp.path(), &names).is_err());
        Ok(())
    }
    #[test]
    fn junit_requires_actual_passing_tests() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let p = tmp.path().join("TEST-suite.xml");
        let required = vec![["Suite".into(), "case".into()]];
        assert!(junit_check(tmp.path(), &required).is_err());
        for child in ["<skipped/>", "<failure/>", "<error/>"] {
            fs::write(&p,format!("<testsuite><testcase classname='pkg.Suite' name='case'>{child}</testcase></testsuite>"))?;
            assert!(junit_check(tmp.path(), &required).is_err());
        }
        fs::write(
            p,
            "<testsuite><testcase classname='pkg.Suite' name='case'/></testsuite>",
        )?;
        junit_check(tmp.path(), &required)?;
        Ok(())
    }
    #[test]
    fn paths_cannot_escape() -> Result<()> {
        let root = tempfile::tempdir()?;
        for p in ["../outside", "/tmp/x", "C:/x", "a/../x"] {
            assert!(path(root.path(), p).is_err());
        }
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn validation_is_isolated_and_cleans_failure() -> Result<()> {
        let root = tempfile::tempdir()?;
        let r = root.path();
        for args in [
            vec!["init"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.invalid"],
        ] {
            ensure!(
                crate::release::git(r, &args)?.status.success(),
                "git fixture failed"
            );
        }
        fs::create_dir_all(r.join(".project/verification"))?;
        for failure in [false, true] {
            fs::write(
                r.join(".project/verification/checks.json"),
                serde_json::to_string(
                    &json!({"release_validation":[["sh","-c",if failure {"echo changed > marker; exit 3"} else {"echo changed > marker"}]]}),
                )?,
            )?;
            ensure!(
                crate::release::git(r, &["add", "."])?.status.success(),
                "add failed"
            );
            ensure!(
                crate::release::git(r, &["commit", "-m", "fixture"])?
                    .status
                    .success(),
                "commit failed"
            );
            let head = crate::release::git(r, &["rev-parse", "HEAD"])?;
            let source = String::from_utf8(head.stdout)?;
            let result = release_validate(r, source.trim());
            assert_eq!(result.is_err(), failure);
            assert!(!r.join("marker").exists());
            let list = crate::release::git(r, &["worktree", "list", "--porcelain"])?;
            assert_eq!(
                String::from_utf8(list.stdout)?.matches("worktree ").count(),
                1
            );
        }
        Ok(())
    }
}
