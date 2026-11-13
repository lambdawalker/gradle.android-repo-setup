use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;
fn fixture(pin: &str) -> TempDir {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join(".project/setup")).unwrap();
    fs::write(
        root.path().join(".project/setup/config.yaml"),
        format!("tooling_ref: {pin}\n"),
    )
    .unwrap();
    root
}
fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
        .arg("--project")
        .arg(root)
        .arg("bootstrap")
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn generated_files_are_checked_read_only_and_pin_changes_require_refresh() {
    let root = fixture("v0.1.14");
    assert!(run(root.path(), &["generate"]).status.success());
    let before = fs::read(root.path().join("setup.sh")).unwrap();
    assert!(run(root.path(), &["check"]).status.success());
    fs::write(root.path().join("setup.sh"), "custom").unwrap();
    assert!(!run(root.path(), &["check"]).status.success());
    assert_eq!(
        fs::read_to_string(root.path().join("setup.sh")).unwrap(),
        "custom"
    );
    assert!(!run(root.path(), &["generate"]).status.success());
    assert!(run(root.path(), &["generate", "--overwrite"])
        .status
        .success());
    assert_eq!(before, fs::read(root.path().join("setup.sh")).unwrap());
    fs::write(
        root.path().join(".project/setup/config.yaml"),
        "tooling_ref: v0.1.15\n",
    )
    .unwrap();
    assert!(!run(root.path(), &["check"]).status.success());
    assert!(run(root.path(), &["generate", "--overwrite"])
        .status
        .success());
    assert!(fs::read_to_string(root.path().join("setup.sh"))
        .unwrap()
        .contains("pin='v0.1.15'"));
}
#[test]
fn unsafe_pin_has_no_side_effects() {
    let root = fixture("'$(touch pwned)'");
    assert!(!run(root.path(), &["generate"]).status.success());
    assert!(!root.path().join("setup.sh").exists());
}
#[test]
fn jitpack_is_opt_in_and_preserves_project_java_setup() {
    let root = fixture("v0.1.14");
    assert!(run(root.path(), &["generate"]).status.success());
    assert!(!root.path().join("jitpack.yml").exists());
    fs::write(
        root.path().join(".project/setup/config.yaml"),
        "tooling_ref: v0.1.14\nbootstrap:\n  jitpack: true\n",
    )
    .unwrap();
    fs::write(
        root.path().join("jitpack.yml"),
        "before_install: [install-java25]\njdk: [openjdk25]\n",
    )
    .unwrap();
    assert!(run(root.path(), &["generate", "--overwrite"])
        .status
        .success());
    assert!(fs::read_to_string(root.path().join("jitpack.yml"))
        .unwrap()
        .contains("install-java25"));
    assert!(run(root.path(), &["check"]).status.success());
}
#[cfg(unix)]
#[test]
fn symlinks_are_rejected_before_writes() {
    let root = fixture("v0.1.14");
    std::os::unix::fs::symlink("outside", root.path().join("setup.sh")).unwrap();
    assert!(!run(root.path(), &["generate", "--overwrite"])
        .status
        .success());
    assert!(!root.path().join("setup.bat").exists());
}
#[cfg(unix)]
#[test]
fn cached_launcher_preserves_arguments_exit_and_checks_version() {
    use std::os::unix::fs::PermissionsExt;
    let root = fixture("v0.1.14");
    assert!(run(root.path(), &["generate"]).status.success());
    let bin = root.path().join(".project/setup/cache/v0.1.14/bin");
    fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("android-repo-setup");
    fs::write(&exe, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'android-repo-setup 0.1.14'; exit 0; fi\nprintf '%s\\n' \"$@\"\nexit 37\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let result = Command::new("sh")
        .arg(root.path().join("setup.sh"))
        .args(["hello world", "two"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(37));
    assert!(String::from_utf8_lossy(&result.stdout).ends_with("hello world\ntwo\n"));
    fs::write(&exe, "#!/bin/sh\necho 'android-repo-setup 0.0.1'\n").unwrap();
    assert!(!Command::new("sh")
        .arg(root.path().join("setup.sh"))
        .status()
        .unwrap()
        .success());
}
#[cfg(unix)]
#[test]
fn checksum_failure_keeps_cache_uninstalled() {
    use std::os::unix::fs::PermissionsExt;
    let root = fixture("v0.1.14");
    assert!(run(root.path(), &["generate"]).status.success());
    let mocks = root.path().join("mocks");
    fs::create_dir(&mocks).unwrap();
    fs::write(mocks.join("curl"), "#!/bin/sh\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then shift; printf garbage > \"$1\"; exit; fi; shift; done\n").unwrap();
    fs::set_permissions(mocks.join("curl"), fs::Permissions::from_mode(0o755)).unwrap();
    let result = Command::new("sh")
        .arg(root.path().join("setup.sh"))
        .env(
            "PATH",
            format!("{}:{}", mocks.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!root.path().join(".project/setup/cache/v0.1.14").exists());
    assert_eq!(
        fs::read_dir(root.path().join(".project/setup/cache"))
            .unwrap()
            .count(),
        0
    );
}

#[cfg(windows)]
#[test]
fn windows_launcher_uses_verified_cache_and_propagates_cli_exit() {
    let pin = format!("v{}", env!("CARGO_PKG_VERSION"));
    let root = fixture(&pin);
    assert!(run(root.path(), &["generate"]).status.success());
    let bin = root.path().join(format!(".project/setup/cache/{pin}/bin"));
    fs::create_dir_all(&bin).unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_android-repo-setup"),
        bin.join("android-repo-setup.exe"),
    )
    .unwrap();
    let success = Command::new("cmd")
        .args(["/c", "setup.bat", "bootstrap", "check"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let failure = Command::new("cmd")
        .args(["/c", "setup.bat", "unknown-command"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(!failure.status.success());
}

#[test]
fn crlf_checkouts_are_equivalent_and_generation_is_lf() {
    let root = fixture("v0.1.14");
    assert!(run(root.path(), &["generate"]).status.success());
    for name in [
        "setup.sh",
        "setup.bat",
        ".project/setup/bootstrap.ps1",
        ".github/workflows/verify-setup.yml",
        ".gitignore",
    ] {
        let file = root.path().join(name);
        let generated = fs::read_to_string(&file).unwrap();
        assert!(!generated.contains('\r'));
        fs::write(file, generated.replace('\n', "\r\n")).unwrap();
    }
    assert!(run(root.path(), &["check"]).status.success());
    assert!(run(root.path(), &["generate"]).status.success());
    for name in [
        "setup.sh",
        "setup.bat",
        ".project/setup/bootstrap.ps1",
        ".github/workflows/verify-setup.yml",
        ".gitignore",
    ] {
        assert!(!fs::read_to_string(root.path().join(name))
            .unwrap()
            .contains('\r'));
    }
}

#[test]
fn documentation_bootstrap_uses_its_own_pin_and_checks_drift() {
    let root = fixture("v0.1.15");
    assert!(run(root.path(), &["generate"]).status.success());
    assert!(!root.path().join("docs.sh").exists());
    fs::create_dir_all(root.path().join(".project/docs")).unwrap();
    fs::write(
        root.path().join(".project/docs/config.yaml"),
        "schema: 1\ntooling_ref: v0.1.16\n",
    )
    .unwrap();
    let out = run(root.path(), &["generate", "--overwrite"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let launcher = fs::read_to_string(root.path().join("docs.sh")).unwrap();
    assert!(launcher.contains("pin='v0.1.16'"));
    assert!(launcher.contains("android-repo-docs"));
    let ignore = fs::read_to_string(root.path().join(".gitignore")).unwrap();
    assert!(ignore.contains("/.project/setup/cache/"));
    assert!(ignore.contains("/.project/docs/cache/"));
    let workflow =
        fs::read_to_string(root.path().join(".github/workflows/verify-setup.yml")).unwrap();
    assert!(
        workflow.contains("          sh ./docs.sh check\n"),
        "opted-in verification must use the standalone docs check:\n{workflow}"
    );
    assert!(workflow.contains("          fetch-depth: 0\n          lfs: true\n"));
    assert!(!workflow.contains("sh ./setup.sh module-release verify"));
    assert!(!workflow.contains("sh ./setup.sh documentation-history verify"));
    assert!(workflow.contains("run: docs.bat bootstrap check\n"));
    assert!(run(root.path(), &["check"]).status.success());
    fs::write(root.path().join("docs.sh"), "changed").unwrap();
    assert!(!run(root.path(), &["check"]).status.success());
}

#[cfg(unix)]
#[test]
fn launcher_makes_pinned_executable_available_to_child_processes() {
    use std::os::unix::fs::PermissionsExt;
    let root = fixture("v0.1.15");
    assert!(run(root.path(), &["generate"]).status.success());
    let bin = root.path().join(".project/setup/cache/v0.1.15/bin");
    fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("android-repo-setup");
    fs::write(&exe, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'android-repo-setup 0.1.15'; exit 0; fi\ncommand -v android-repo-setup\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let result = Command::new("sh")
        .arg(root.path().join("setup.sh"))
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        exe.to_str().unwrap()
    );
}

#[test]
fn docs_cache_rule_is_separate_after_unterminated_setup_rule() {
    let root = fixture("v0.1.15");
    fs::create_dir_all(root.path().join(".project/docs")).unwrap();
    fs::write(
        root.path().join(".project/docs/config.yaml"),
        "tooling_ref: v0.1.16\n",
    )
    .unwrap();
    fs::write(root.path().join(".gitignore"), "/.project/setup/cache/").unwrap();
    assert!(run(root.path(), &["generate"]).status.success());
    assert_eq!(
        fs::read_to_string(root.path().join(".gitignore")).unwrap(),
        "/.project/setup/cache/\n/.project/docs/cache/\n"
    );
}
