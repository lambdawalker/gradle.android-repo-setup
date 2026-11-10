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
fn cli_matches_python_workflows() {
    let root = fixture();
    let generated = run(root.path(), &[]);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let output = root.path().join(".github/workflows");
    let hashes = include_str!("fixtures/python-workflow-hashes.txt");
    assert_eq!(
        fs::read_dir(&output).unwrap().count(),
        hashes.lines().count()
    );
    for line in hashes.lines() {
        let (expected, name) = line.split_once(' ').unwrap();
        let bytes = fs::read(output.join(name)).unwrap();
        assert_eq!(expected, format!("{:x}", Sha256::digest(&bytes)), "{name}");
    }
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
