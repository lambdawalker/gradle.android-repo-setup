use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;
fn git(root: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap()
        .success());
}
fn fixture(contract: &str, data: &[u8]) -> TempDir {
    let t = TempDir::new().unwrap();
    let r = t.path();
    git(r, &["init", "-q"]);
    git(r, &["config", "user.name", "Test"]);
    git(r, &["config", "user.email", "test@example.invalid"]);
    fs::create_dir_all(r.join(".project/setup")).unwrap();
    fs::create_dir_all(r.join("model/src/main/assets")).unwrap();
    fs::write(r.join(".project/setup/config.yaml"),format!("repository: example/repo\ntitle: Test\ntooling_ref: v0.1.13\nmodules:\n  model: Model\nandroid_modules:\n  model:\n    path: ':model'\n    directory: model\n    kind: library\n    selected: true\nmodule_verification:\n  model:\n    required_api_artifacts: []\n{contract}")).unwrap();
    fs::write(r.join("model/src/main/assets/test.tflite"), data).unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "fixture"]);
    t
}
fn run(r: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
        .arg("--project")
        .arg(r)
        .args(["assets", "prepare", "--module", "model"])
        .output()
        .unwrap()
}
const ASSETS:&str="    assets:\n      - directory: model/src/main/assets\n        include: ['**/*.tflite']\n        source: git\n        validator: tflite\n";
#[test]
fn prepares_committed_assets_and_rejects_changed_bytes() {
    let t = fixture(ASSETS, b"\0\0\0\0TFL3model");
    let o = run(t.path());
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    fs::write(
        t.path().join("model/src/main/assets/test.tflite"),
        b"\0\0\0\0TFL3changed",
    )
    .unwrap();
    assert!(!run(t.path()).status.success());
}
#[test]
fn rejects_invalid_format() {
    let t = fixture(ASSETS, b"not a model");
    assert!(!run(t.path()).status.success());
}
#[test]
fn legacy_assets_still_prepare() {
    let t = fixture(
        "    required_assets:\n      directory: model/src/main/assets\n      suffix: .tflite\n",
        b"legacy bytes",
    );
    let o = run(t.path());
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}
#[test]
fn rejects_untracked_matching_assets() {
    let t = fixture(ASSETS, b"\0\0\0\0TFL3model");
    fs::write(
        t.path().join("model/src/main/assets/extra.tflite"),
        b"\0\0\0\0TFL3extra",
    )
    .unwrap();
    assert!(!run(t.path()).status.success());
}
#[test]
fn configuration_is_read_from_commit() {
    let t = fixture(ASSETS, b"bad model");
    fs::write(
        t.path().join(".project/setup/config.yaml"),
        "modules: {model: Model}\n",
    )
    .unwrap();
    assert!(!run(t.path()).status.success());
}
#[test]
fn optional_missing_set_and_explicit_empty_files_are_allowed() {
    let t=fixture("    assets:\n      - directory: absent\n        required: false\n      - directory: model/src/main/assets\n        allow_empty_files: true\n",b"");
    let o = run(t.path());
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}
#[test]
fn required_missing_set_fails() {
    let t = fixture("    assets:\n      - directory: absent\n", b"data");
    assert!(!run(t.path()).status.success());
}
#[test]
fn rejects_mixed_legacy_and_new_contracts() {
    let t=fixture(&format!("{ASSETS}    required_assets:\n      directory: model/src/main/assets\n      suffix: .tflite\n"),b"\0\0\0\0TFL3model");
    assert!(!run(t.path()).status.success());
}
#[test]
fn rejects_unsafe_directories() {
    for directory in ["../outside", "/absolute", "C:/outside", "model/../assets"] {
        let t = fixture(
            &format!("    assets:\n      - directory: {directory}\n"),
            b"data",
        );
        assert!(!run(t.path()).status.success(), "{directory}");
    }
}
#[test]
fn rejects_archive_path_collisions() {
    let t=fixture("    assets:\n      - directory: model/src/main/assets\n      - directory: model/src/main/assets\n",b"data");
    assert!(!run(t.path()).status.success());
}
#[cfg(unix)]
#[test]
fn rejects_worktree_symlink() {
    let t = fixture(ASSETS, b"\0\0\0\0TFL3model");
    let path = t.path().join("model/src/main/assets/test.tflite");
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", path).unwrap();
    assert!(!run(t.path()).status.success());
}
#[test]
fn unresolved_lfs_pointer_requires_opt_in() {
    let t=fixture(ASSETS,b"version https://git-lfs.github.com/spec/v1\noid sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nsize 12\n");
    assert!(!run(t.path()).status.success());
}

#[test]
fn hydrates_only_selected_lfs_assets_and_uses_pointer_hashes() {
    use sha2::{Digest, Sha256};
    let data = b"\0\0\0\0TFL3model";
    let origin = fixture(
        &ASSETS.replace("source: git", "source: git\n        hydrate_lfs: true"),
        data,
    );
    let r = origin.path();
    git(r, &["lfs", "install", "--local"]);
    git(r, &["lfs", "track", "*.tflite"]);
    fs::write(r.join("unrelated.tflite"), b"unrelated large asset").unwrap();
    git(r, &["add", "."]);
    git(r, &["add", "--renormalize", "."]);
    git(r, &["commit", "-qm", "LFS"]);
    let checkout = tempfile::tempdir().unwrap();
    let result = Command::new("git")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args([
            "clone",
            "--quiet",
            r.to_str().unwrap(),
            checkout.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let selected = checkout.path().join("model/src/main/assets/test.tflite");
    assert!(fs::read(&selected)
        .unwrap()
        .starts_with(b"version https://git-lfs"));
    let output = run(checkout.path());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(&selected).unwrap(), data);
    assert!(fs::read(checkout.path().join("unrelated.tflite"))
        .unwrap()
        .starts_with(b"version https://git-lfs"));
    let manifest: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        manifest["assets/test.tflite"]["sha256"],
        format!("{:x}", Sha256::digest(data))
    );
    assert_eq!(manifest["assets/test.tflite"]["size"], data.len());
    fs::write(selected, b"\0\0\0\0TFL3wrong").unwrap();
    assert!(!run(checkout.path()).status.success());
}

#[test]
fn combines_multiple_generic_asset_sets() {
    let t = fixture("    assets:\n      - directory: model/src/main/assets\n        include: ['**/*.tflite']\n      - directory: dictionaries\n        include: ['*.txt']\n        archive_directory: assets/dictionaries\n", b"generic binary");
    fs::create_dir(t.path().join("dictionaries")).unwrap();
    fs::write(t.path().join("dictionaries/words.txt"), b"hello").unwrap();
    git(t.path(), &["add", "."]);
    git(t.path(), &["commit", "-qm", "dictionary"]);
    let output = run(t.path());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(manifest.as_object().unwrap().len(), 2);
    assert_eq!(manifest["assets/dictionaries/words.txt"]["size"], 5);
}

#[test]
fn optional_set_cannot_hide_untracked_file_behind_another_set() {
    let t = fixture("    assets:\n      - directory: model/src/main/assets\n      - directory: optional\n        required: false\n", b"generic");
    fs::create_dir(t.path().join("optional")).unwrap();
    fs::write(t.path().join("optional/test.tflite"), b"untracked").unwrap();
    assert!(!run(t.path()).status.success());
}
