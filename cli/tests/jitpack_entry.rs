#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().into()
}

#[test]
fn jitpack_entry_validates_the_tag_and_source_before_invoking_gradle() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir_all(root.join(".project/setup")).unwrap();
    fs::write(
        root.join(".project/setup/config.yaml"),
        "modules:\n  demo: Demo\n",
    )
    .unwrap();
    let wrapper = root.join("gradlew");
    fs::write(
        &wrapper,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > gradle-invocation\n",
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    git(root, &["init"]);
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "Fixture",
        ],
    );
    git(root, &["tag", "demo/v1.2.3-jitpack"]);
    let run = |tag: &str, source: &str| {
        Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
            .arg("--project")
            .arg(root)
            .arg("jitpack_build")
            .env("VERSION", tag)
            .env("GIT_COMMIT", source)
            .output()
            .unwrap()
    };
    let source = git(root, &["rev-parse", "HEAD"]);
    assert!(!run("demo~v1.2.4-jitpack", &source).status.success());
    assert!(!run("demo~v1.2.3-jitpack", &"a".repeat(40)).status.success());
    let output_file = root.join("gradle-invocation");
    assert!(!output_file.exists());
    let result = run("demo~v1.2.3-jitpack", &source);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read_to_string(&output_file).unwrap(), "--no-daemon\n:demo:publishToMavenLocal\n-PjitpackBuild=true\n-PreleaseModule=demo\n-PreleaseVersion=1.2.3\n-PjitpackScopedTag=true\n");
    // Invalid committed assets prevent the Gradle wrapper from starting.
    fs::remove_file(&output_file).unwrap();
    fs::write(root.join(".project/setup/config.yaml"), "modules:\n  demo: Demo\nmodule_verification:\n  demo:\n    required_api_artifacts: []\n    assets:\n      - directory: missing-assets\n").unwrap();
    git(root, &["add", ".project/setup/config.yaml"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "Missing assets",
        ],
    );
    git(root, &["tag", "demo/v1.2.4-jitpack"]);
    let source = git(root, &["rev-parse", "HEAD"]);
    let result = run("demo~v1.2.4-jitpack", &source);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Required asset set"));
    assert!(!output_file.exists());
}
