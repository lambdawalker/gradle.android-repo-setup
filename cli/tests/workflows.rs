use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn fixture() -> TempDir {
    let root = TempDir::new().unwrap();
    let setup = root.path().join(".project/setup");
    fs::create_dir_all(&setup).unwrap();
    fs::write(
        setup.join("config.yaml"),
        include_str!("fixtures/config.yaml"),
    )
    .unwrap();
    root
}

fn run(root: &Path, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
        .arg("--project")
        .arg(root)
        .arg("workflows")
        .args(flags)
        .output()
        .unwrap()
}

#[test]
fn cli_generates_native_workflows_and_tracks_their_content() {
    let root = fixture();
    let generated = run(root.path(), &[]);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let output = root.path().join(".github/workflows");
    let manifest = fs::read(root.path().join(".project/setup/workflow-generation.json")).unwrap();
    let hashes: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&manifest).unwrap();
    assert_eq!(hashes.len(), 10);
    assert_eq!(fs::read_dir(&output).unwrap().count(), hashes.len());
    for (name, expected) in hashes {
        let contents = fs::read_to_string(output.join(&name)).unwrap();
        assert_eq!(
            expected,
            format!("{:x}", Sha256::digest(contents.as_bytes())),
            "{name}"
        );
        assert!(
            !contents.contains("setup-python") && !contents.contains("pip install"),
            "{name}"
        );
        if contents.contains("android-repo-setup ") {
            assert!(contents.contains("cargo install --locked --git"), "{name}");
            assert!(
                contents.contains("https://github.com/lambdawalker/gradle.android-repo-setup.git"),
                "{name}"
            );
        }
    }
}

#[test]
fn cli_updates_the_tooling_pin_without_reformatting_other_yaml() {
    let root = fixture();
    let pin = "1234567890abcdef1234567890abcdef12345678";
    let config = root.path().join(".project/setup/config.yaml");
    let original = fs::read_to_string(&config)
        .unwrap()
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
    fs::write(&config, &original).unwrap();
    assert!(
        !run(root.path(), &["--overwrite", "--set-tooling-ref", "main"])
            .status
            .success()
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    let result = run(root.path(), &["--overwrite", "--set-tooling-ref", pin]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        original.replace("tooling_ref: v0.1.4", &format!("tooling_ref: '{pin}'"))
    );
    assert!(
        fs::read_to_string(root.path().join(".github/workflows/documentation.yml"))
            .unwrap()
            .contains(pin)
    );
}

#[test]
fn cli_preserves_local_edits_until_overwrite() {
    let root = fixture();
    assert!(!run(root.path(), &["--check"]).status.success());
    assert!(!root.path().join(".github").exists());
    assert!(!root
        .path()
        .join(".project/setup/workflow-generation.lock")
        .exists());

    assert!(run(root.path(), &[]).status.success());
    assert!(run(root.path(), &["--check"]).status.success());
    let workflow = root.path().join(".github/workflows/documentation.yml");
    fs::write(&workflow, "# local edit\n").unwrap();
    assert!(!run(root.path(), &[]).status.success());
    assert!(run(root.path(), &["--only-missing"]).status.success());
    assert_eq!(fs::read_to_string(&workflow).unwrap(), "# local edit\n");
    assert!(run(root.path(), &["--overwrite"]).status.success());
    assert!(run(root.path(), &["--check"]).status.success());
}

#[cfg(unix)]
#[test]
fn cli_refuses_symlinked_output_directory() {
    use std::os::unix::fs::symlink;
    let root = fixture();
    let other = TempDir::new().unwrap();
    symlink(other.path(), root.path().join(".github")).unwrap();
    assert!(!run(root.path(), &["--overwrite"]).status.success());
    assert_eq!(fs::read_dir(other.path()).unwrap().count(), 0);
}

#[test]
fn cli_uses_the_configured_app_module_and_defaults_to_sample() {
    for app_module in [None, Some("app")] {
        let root = fixture();
        if let Some(app_module) = app_module {
            let config = root.path().join(".project/setup/config.yaml");
            let content = fs::read_to_string(&config).unwrap();
            fs::write(
                config,
                content.replace(
                    "workflows:\n",
                    &format!("workflows:\n  app_module: {app_module}\n"),
                ),
            )
            .unwrap();
        }
        let result = run(root.path(), &[]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let module = app_module.unwrap_or("sample");
        let output = root.path().join(".github/workflows");
        let screenshots = fs::read_to_string(output.join("screenshots.yml")).unwrap();
        assert!(screenshots.contains(&format!(":{module}:assembleDebug :{module}:lintDebug")));
        let apk = fs::read_to_string(output.join("build-signed-demo-apk.yml")).unwrap();
        assert!(apk.contains(&format!(":{module}:assembleRelease")));
        assert!(apk.contains(&format!("apks=({module}/build/outputs/apk/release/")));
        assert!(apk.contains(&format!("signed-demo/demo-{module}-")));
        assert!(run(root.path(), &["--check"]).status.success());
    }
}

#[test]
fn cli_rejects_app_module_paths_and_shell_expressions_before_writing() {
    for module in ["../app", "app;echo bad", "$(whoami)", "app:child"] {
        let root = fixture();
        let config = root.path().join(".project/setup/config.yaml");
        let content = fs::read_to_string(&config).unwrap();
        fs::write(
            config,
            content.replace(
                "workflows:\n",
                &format!("workflows:\n  app_module: '{module}'\n"),
            ),
        )
        .unwrap();
        let result = run(root.path(), &[]);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("workflows.app_module"));
        assert!(!root.path().join(".github/workflows").exists());
    }
}
