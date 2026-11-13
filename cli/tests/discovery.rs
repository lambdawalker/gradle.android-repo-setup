use std::{fs, process::Command};
use tempfile::tempdir;

#[cfg(unix)]
#[test]
fn discovery_uses_gradle_report_and_preserves_exclusions_and_new_defaults() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".project/setup")).unwrap();
    fs::write(root.join(".project/setup/config.yaml"), "repository: example/project\ntooling_ref: v0.1.10\nmodules: {}\nworkflows: {module: old, title: Project}\n").unwrap();
    // Fake only the process boundary: exercise the real report parser and persistence.
    fs::write(root.join("gradlew"), r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
 case "$1" in -DandroidRepoSetup.report=*) report="${1#*=}";; esac
 shift
done
cat > "$report" <<'JSON'
{"schema":1,"modules":[{"path":":libs:vision","directory":"components/detection","kind":"library","namespace":"com.example.vision","application_ids":[],"screenshots":false},{"path":":demo","directory":"mobile","kind":"application","namespace":"com.example.demo","application_ids":["com.example.demo"],"screenshots":false},{"path":":support","directory":"support","kind":"other","application_ids":[],"screenshots":false}]}
JSON
"#).unwrap();
    fs::set_permissions(root.join("gradlew"), fs::Permissions::from_mode(0o755)).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
            .arg("--project")
            .arg(root)
            .args(args)
            .output()
            .unwrap()
    };
    let result = run(&["discover", "--save"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let path = root.join(".project/setup/config.yaml");
    let mut config: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        config["android_modules"]["libs--vision"]["path"],
        ":libs:vision"
    );
    assert_eq!(
        config["android_modules"]["libs--vision"]["directory"],
        "components/detection"
    );
    assert_eq!(config["android_modules"]["demo"]["selected"], true);
    assert!(config["android_modules"]["support"].is_null());
    config["android_modules"]["demo"]["selected"] = false.into();
    fs::write(&path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    assert!(run(&["discover", "--save"]).status.success());
    let config: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(config["android_modules"]["demo"]["selected"], false);
    assert_eq!(config["android_modules"]["libs--vision"]["selected"], true);
    let result = run(&["module-path", "libs--vision"]);
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        ":libs:vision"
    );
    let before = fs::read(&path).unwrap();
    fs::write(root.join("gradlew"), "#!/bin/sh\nexit 1\n").unwrap();
    assert!(!run(&["discover", "--save"]).status.success());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn discovery_can_bootstrap_a_github_project_without_setup_files() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let root = temp.path();
    for args in [
        vec!["init"],
        vec![
            "remote",
            "add",
            "origin",
            "git@github.com:example/android-app.git",
        ],
    ] {
        assert!(Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    fs::write(root.join("gradlew"), "#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in -DandroidRepoSetup.report=*) report=\"${arg#*=}\";; esac; done\nprintf '%s' '{\"schema\":1,\"modules\":[{\"path\":\":app\",\"directory\":\"app\",\"kind\":\"application\"}]}' > \"$report\"\n").unwrap();
    fs::set_permissions(root.join("gradlew"), fs::Permissions::from_mode(0o755)).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
        .arg("--project")
        .arg(root)
        .args(["discover", "--save"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let config: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&fs::read(root.join(".project/setup/config.yaml")).unwrap())
            .unwrap();
    assert_eq!(config["repository"], "example/android-app");
    assert_eq!(config["android_modules"]["app"]["selected"], true);
    assert_eq!(config["modules"].as_mapping().unwrap().len(), 0);
}

#[cfg(unix)]
#[test]
fn saved_selection_immediately_updates_gradle_inventory() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".project/setup")).unwrap();
    fs::write(root.join(".project/setup/config.yaml"), "repository: example/project\nmodules: {core: Core}\nandroid_modules:\n  core: {path: ':core', directory: core, kind: library, selected: false}\n").unwrap();
    fs::write(
        root.join(".project/setup/modules.properties"),
        "core.path=:core\ncore.kind=library\ncore.selected=true\n",
    )
    .unwrap();
    fs::write(root.join("gradlew"), "#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in -DandroidRepoSetup.report=*) report=\"${arg#*=}\";; esac; done\nprintf '%s' '{\"schema\":1,\"modules\":[{\"path\":\":core\",\"directory\":\"core\",\"kind\":\"library\"}]}' > \"$report\"\n").unwrap();
    fs::set_permissions(root.join("gradlew"), fs::Permissions::from_mode(0o755)).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
        .arg("--project")
        .arg(root)
        .args(["discover", "--save"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        fs::read_to_string(root.join(".project/setup/modules.properties"))
            .unwrap()
            .contains("core.selected=false")
    );
}

#[cfg(unix)]
#[test]
fn discovery_saves_root_jvm_history_without_android_metadata_or_inferred_contract() {
    use std::os::unix::fs::PermissionsExt;
    let project = tempdir().unwrap();
    let root = project.path();
    fs::create_dir_all(root.join(".project/setup")).unwrap();
    fs::write(root.join(".project/setup/config.yaml"), "repository: example/coordinates\ntooling_ref: v0.1.25\nmodules: {}\nworkflows: {title: Coordinates}\n").unwrap();
    fs::write(
        root.join("gradlew"),
        r#"#!/bin/sh
for arg in "$@"; do case "$arg" in -DandroidRepoSetup.report=*) report="${arg#*=}";; esac; done
printf '%s' '{"schema":1,"modules":[{"path":":","directory":"","kind":"jvm-library"}]}' > "$report"
"#,
    )
    .unwrap();
    fs::set_permissions(root.join("gradlew"), fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
        .arg("--project")
        .arg(root)
        .args(["discover", "--save"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&fs::read(root.join(".project/setup/config.yaml")).unwrap())
            .unwrap();
    assert_eq!(config["android_modules"]["root"]["kind"], "jvm-library");
    assert_eq!(config["android_modules"]["root"]["directory"], ".");
    assert!(config["android_modules"]["root"]["namespace"].is_null());
    assert_eq!(config["modules"]["root"], "root");
    assert!(config["module_verification"].is_null());
    let inventory = fs::read_to_string(root.join(".project/setup/modules.properties")).unwrap();
    assert!(inventory.contains("root.path=:\n"));
    assert!(inventory.contains("root.kind=jvm-library\n"));
    assert!(root.join(".project/setup/repositories.yml").exists());
}
