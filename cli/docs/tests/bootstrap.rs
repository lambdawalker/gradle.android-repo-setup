use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;
fn fixture(pin: &str) -> TempDir {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join(".project/docs")).unwrap();
    fs::write(
        root.path().join(".project/docs/config.yaml"),
        format!("schema: 1\ntooling_ref: {pin}\n"),
    )
    .unwrap();
    root
}
fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_android-repo-docs"))
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
    let before = fs::read(root.path().join("docs.sh")).unwrap();
    assert!(run(root.path(), &["check"]).status.success());
    fs::write(root.path().join("docs.sh"), "custom").unwrap();
    assert!(!run(root.path(), &["check"]).status.success());
    assert_eq!(
        fs::read_to_string(root.path().join("docs.sh")).unwrap(),
        "custom"
    );
    assert!(!run(root.path(), &["generate"]).status.success());
    assert!(run(root.path(), &["generate", "--overwrite"])
        .status
        .success());
    assert_eq!(before, fs::read(root.path().join("docs.sh")).unwrap());
    fs::write(
        root.path().join(".project/docs/config.yaml"),
        "tooling_ref: v0.1.15\n",
    )
    .unwrap();
    assert!(!run(root.path(), &["check"]).status.success());
    assert!(run(root.path(), &["generate", "--overwrite"])
        .status
        .success());
    assert!(fs::read_to_string(root.path().join("docs.sh"))
        .unwrap()
        .contains("pin='v0.1.15'"));
}
#[test]
fn unsafe_pin_has_no_side_effects() {
    let root = fixture("'$(touch pwned)'");
    assert!(!run(root.path(), &["generate"]).status.success());
    assert!(!root.path().join("docs.sh").exists());
}
#[cfg(unix)]
#[test]
fn symlinks_are_rejected_before_writes() {
    let root = fixture("v0.1.14");
    std::os::unix::fs::symlink("outside", root.path().join("docs.sh")).unwrap();
    assert!(!run(root.path(), &["generate", "--overwrite"])
        .status
        .success());
    assert!(!root.path().join("docs.bat").exists());
}
#[cfg(unix)]
#[test]
fn cached_launcher_preserves_arguments_exit_and_checks_version() {
    use std::os::unix::fs::PermissionsExt;
    let root = fixture("v0.1.14");
    assert!(run(root.path(), &["generate"]).status.success());
    let bin = root.path().join(".project/docs/cache/v0.1.14/bin");
    fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("android-repo-docs");
    fs::write(&exe, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'android-repo-docs 0.1.14'; exit 0; fi\nprintf '%s\\n' \"$@\"\nexit 37\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let result = Command::new("sh")
        .arg(root.path().join("docs.sh"))
        .args(["hello world", "two"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(37));
    assert!(String::from_utf8_lossy(&result.stdout).ends_with("hello world\ntwo\n"));
    fs::write(&exe, "#!/bin/sh\necho 'android-repo-docs 0.0.1'\n").unwrap();
    assert!(!Command::new("sh")
        .arg(root.path().join("docs.sh"))
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
        .arg(root.path().join("docs.sh"))
        .env(
            "PATH",
            format!("{}:{}", mocks.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!root.path().join(".project/docs/cache/v0.1.14").exists());
    assert_eq!(
        fs::read_dir(root.path().join(".project/docs/cache"))
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
    let bin = root.path().join(format!(".project/docs/cache/{pin}/bin"));
    fs::create_dir_all(&bin).unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_android-repo-docs"),
        bin.join("android-repo-docs.exe"),
    )
    .unwrap();
    let success = Command::new("cmd")
        .args(["/c", "docs.bat", "bootstrap", "check"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let failure = Command::new("cmd")
        .args(["/c", "docs.bat", "unknown-command"])
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
        "docs.sh",
        "docs.bat",
        ".project/docs/bootstrap.ps1",
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
        "docs.sh",
        "docs.bat",
        ".project/docs/bootstrap.ps1",
        ".gitignore",
    ] {
        assert!(!fs::read_to_string(root.path().join(name))
            .unwrap()
            .contains('\r'));
    }
}

fn cached_docs(root: &Path, pin: &str) {
    let bin = root.join(format!(".project/docs/cache/{pin}/bin"));
    fs::create_dir_all(&bin).unwrap();
    let name = if cfg!(windows) {
        "android-repo-docs.exe"
    } else {
        "android-repo-docs"
    };
    fs::copy(env!("CARGO_BIN_EXE_android-repo-docs"), bin.join(name)).unwrap();
}

fn launcher(root: &Path) -> Command {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("sh");
        command.arg(root.join("docs.sh"));
        command
    };
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/c", "docs.bat"]);
        command
    };
    command.current_dir(root);
    command
}

#[test]
fn cached_launchers_accept_supported_yaml_pin_forms_and_reject_changed_pins() {
    let pin = format!("v{}", env!("CARGO_PKG_VERSION"));
    let variants = [
        format!("tooling_ref: {pin}\n"),
        format!("tooling_ref:   {pin}    # pinned docs tool\n"),
        format!("\"tooling_ref\" : \"{pin}\" # pinned docs tool\n"),
        format!("'tooling_ref': '{pin}'\n"),
        format!("tooling_ref:\t{pin}\t# pinned docs tool\r\n"),
    ];
    for variant in variants {
        let root = fixture(&pin);
        fs::write(
            root.path().join(".project/docs/config.yaml"),
            format!("schema: 1\r\n{variant}"),
        )
        .unwrap();
        let generated = run(root.path(), &["generate"]);
        assert!(
            generated.status.success(),
            "{variant:?}: {}",
            String::from_utf8_lossy(&generated.stderr)
        );
        assert!(run(root.path(), &["check"]).status.success(), "{variant:?}");
        cached_docs(root.path(), &pin);
        let launched = launcher(root.path())
            .args(["bootstrap", "check"])
            .output()
            .unwrap();
        assert!(
            launched.status.success(),
            "{variant:?}: {}",
            String::from_utf8_lossy(&launched.stderr)
        );
        fs::write(
            root.path().join(".project/docs/config.yaml"),
            variant.replace(&pin, "v99.98.97"),
        )
        .unwrap();
        let changed = launcher(root.path())
            .args(["bootstrap", "check"])
            .output()
            .unwrap();
        assert!(
            !changed.status.success(),
            "changed pin accepted: {variant:?}"
        );
        assert!(String::from_utf8_lossy(&changed.stderr).contains("tooling_ref changed"));
        assert!(!root.path().join(".project/docs/cache/v99.98.97").exists());
    }
}

#[test]
fn unsupported_yaml_pin_serialization_is_rejected_before_writes() {
    for text in [
        "tooling_ref: >-\n  v0.1.15\n",
        "tooling_ref: \"v0.1.\\x31\\x35\"\n",
        "{schema: 1, tooling_ref: v0.1.15}\n",
        "tooling_ref: &pin v0.1.15\n",
    ] {
        let root = fixture("v0.1.15");
        fs::write(root.path().join(".project/docs/config.yaml"), text).unwrap();
        for operation in ["generate", "check"] {
            let result = run(root.path(), &[operation]);
            assert!(!result.status.success(), "{operation} accepted {text:?}");
            assert!(
                String::from_utf8_lossy(&result.stderr).contains("single top-level line"),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        assert!(!root.path().join("docs.sh").exists());
        assert!(!root.path().join("docs.bat").exists());
    }
}

#[test]
fn cached_launcher_exposes_docs_on_child_path() {
    let pin = format!("v{}", env!("CARGO_PKG_VERSION"));
    let root = fixture(&pin);
    fs::write(root.path().join(".project/docs/config.yaml"), format!("schema: 1\ntooling_ref: {pin}\nrepository: example/docs\ntitle: Docs\nmodules: {{}}\nsite:\n  build: [android-repo-docs, --version]\n")).unwrap();
    assert!(run(root.path(), &["generate"]).status.success());
    cached_docs(root.path(), &pin);
    #[cfg(unix)]
    let clean_path = std::ffi::OsString::from("/usr/bin:/bin");
    #[cfg(windows)]
    let clean_path = {
        let windows = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        std::env::join_paths([
            windows.join("System32"),
            windows.join("System32/WindowsPowerShell/v1.0"),
        ])
        .unwrap()
    };
    let output = launcher(root.path())
        .arg("build")
        .env("PATH", clean_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("android-repo-docs {}", env!("CARGO_PKG_VERSION"))
    );
}
