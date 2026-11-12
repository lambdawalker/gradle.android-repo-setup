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
    assert_eq!(hashes.len(), 12);
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

fn app_module_fixture(app_module: Option<&str>, newline: &str) -> TempDir {
    let root = fixture();
    let path = root.path().join(".project/setup/config.yaml");
    let mut config: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    if let Some(app_module) = app_module {
        config["workflows"]["app_module"] = app_module.into();
    }
    let serialized = serde_yaml_ng::to_string(&config).unwrap();
    fs::write(path, serialized.replace('\n', newline)).unwrap();
    root
}

#[test]
fn cli_uses_the_configured_app_module_and_defaults_to_sample() {
    for newline in ["\n", "\r\n"] {
        for app_module in [None, Some("app")] {
            let root = app_module_fixture(app_module, newline);
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
}

#[test]
fn cli_rejects_app_module_paths_and_shell_expressions_before_writing() {
    for newline in ["\n", "\r\n"] {
        for module in ["../app", "app;echo bad", "$(whoami)", "app:child"] {
            let root = app_module_fixture(Some(module), newline);
            let result = run(root.path(), &[]);
            assert!(!result.status.success());
            assert!(String::from_utf8_lossy(&result.stderr).contains("workflows.app_module"));
            assert!(!root.path().join(".github/workflows").exists());
        }
    }
}

#[test]
fn reconciliation_workflow_defaults_to_preview_and_serializes_with_publication() {
    let root = fixture();
    let generated = run(root.path(), &[]);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let output = root.path().join(".github/workflows");
    let cleanup: serde_yaml_ng::Value = serde_yaml_ng::from_str(
        &fs::read_to_string(output.join("reconcile-failed-jitpack.yml")).unwrap(),
    )
    .unwrap();
    let publish: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(output.join("publish-library.yml")).unwrap())
            .unwrap();
    assert_eq!(
        cleanup["on"]["workflow_dispatch"]["inputs"]["apply"]["default"],
        false
    );
    assert_eq!(cleanup["concurrency"]["cancel-in-progress"], false);
    let groups: Vec<_> = publish["jobs"]
        .as_mapping()
        .unwrap()
        .values()
        .filter_map(|job| job["concurrency"]["group"].as_str())
        .collect();
    assert!(groups.contains(&cleanup["concurrency"]["group"].as_str().unwrap()));
    assert_eq!(
        cleanup["jobs"]["reconcile"]["if"],
        "github.ref == 'refs/heads/main'"
    );
    let steps = cleanup["jobs"]["reconcile"]["steps"].as_sequence().unwrap();
    let command = steps.last().unwrap()["run"].as_str().unwrap();
    assert!(command.contains("if [[ \"$APPLY\" == true ]]"));
    assert!(command.contains("module_release reconcile-failed-jitpack"));
    assert!(!command.contains("${{ inputs."));
}

#[test]
fn reconciliation_install_uses_tag_or_revision_for_the_selected_tooling_pin() {
    for (pin, flag) in [
        ("v0.1.6", "--tag"),
        ("1234567890abcdef1234567890abcdef12345678", "--rev"),
    ] {
        let root = fixture();
        let result = run(root.path(), &["--overwrite", "--set-tooling-ref", pin]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            &fs::read_to_string(
                root.path()
                    .join(".github/workflows/reconcile-failed-jitpack.yml"),
            )
            .unwrap(),
        )
        .unwrap();
        let install = &workflow["jobs"]["reconcile"]["steps"][1];
        assert_eq!(install["env"]["TOOLING_REF"], pin);
        let script = install["run"].as_str().unwrap();
        assert!(script.contains("ref=(--tag \"$TOOLING_REF\")"));
        assert!(script.contains("ref=(--rev \"$TOOLING_REF\")"));
        assert!(script.contains("\"${ref[@]}\" android-repo-setup"));
        // The generated job uses Ubuntu. Execute its selection with a harmless
        // Cargo stub on Unix, without downloading or installing anything.
        #[cfg(unix)]
        {
            let output = Command::new("bash")
                .arg("-c")
                .arg(format!("cargo() {{ printf '%s\\n' \"$@\"; }}\n{script}"))
                .env("TOOLING_REF", pin)
                .output()
                .unwrap();
            assert!(output.status.success());
            let arguments = String::from_utf8(output.stdout).unwrap();
            assert!(arguments.contains(&format!("{flag}\n{pin}\nandroid-repo-setup\n")));
        }
        #[cfg(not(unix))]
        let _ = flag;
    }
}

#[test]
fn workflow_contract_defaults_and_overrides_preserve_command_scalars() {
    for customized in [false, true] {
        let root = fixture();
        if customized {
            let path = root.path().join(".project/setup/config.yaml");
            let mut config: serde_yaml_ng::Value =
                serde_yaml_ng::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            config["verification"] = serde_yaml_ng::from_str("class_prefix: com/apexfission/android/permissions/\nrequired_api_artifacts: [permissions]").unwrap();
            config["workflows"]["java_version"] = "25".into();
            config["workflows"]["screenshot_count"] = 14.into();
            config["workflows"]["release_validation_command"] =
                "bash scripts/validate-release-source.sh".into();
            config["workflows"]["documentation_command"] =
                "npm --prefix sites ci && npm --prefix sites run check".into();
            config["workflows"]["screenshot_check_command"] =
                "node sites/scripts/screenshots.mjs manifest".into();
            fs::write(path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
        }
        let result = run(root.path(), &[]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = root.path().join(".github/workflows");
        let screenshots = fs::read_to_string(output.join("screenshots.yml")).unwrap();
        assert!(screenshots.starts_with("name: Verify Demo Common"));
        assert!(screenshots.contains(if customized { "-eq 14" } else { "-eq 5" }));
        for name in [
            "screenshots.yml",
            "unit-tests.yml",
            "publish-library.yml",
            "publish-jitpack.yml",
            "build-signed-demo-apk.yml",
            "verify-release-tooling.yml",
        ] {
            let contents = fs::read_to_string(output.join(name)).unwrap();
            assert!(
                contents.contains(if customized {
                    "            25\n"
                } else {
                    "            21\n"
                }),
                "{name}"
            );
        }
        for name in ["publish-library.yml", "publish-jitpack.yml"] {
            let contents = fs::read_to_string(output.join(name)).unwrap();
            let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(&contents).unwrap();
            let steps = workflow["jobs"]
                .as_mapping()
                .unwrap()
                .values()
                .filter_map(|job| job["steps"].as_sequence())
                .flat_map(|steps| steps.iter())
                .collect::<Vec<_>>();
            let validation = steps
                .iter()
                .position(|step| step["name"] == "Validate immutable release source")
                .unwrap();
            let preparation = steps
                .iter()
                .position(|step| step["name"] == "Prepare immutable release assets")
                .unwrap();
            assert!(preparation < validation);
            assert_eq!(steps[preparation]["run"], "android-repo-setup assets prepare --module \"$RELEASE_MODULE\" --source \"$SOURCE_SHA\"");
            assert_eq!(
                steps[preparation]["env"]["RELEASE_MODULE"],
                "${{ inputs.module }}"
            );
            assert_eq!(
                steps[validation]["run"],
                if customized {
                    "bash scripts/validate-release-source.sh"
                } else {
                    "true"
                }
            );
            assert_eq!(
                steps[validation]["if"],
                "steps.release.outputs.skip != 'true'"
            );
            assert_eq!(
                steps[validation]["env"]["SOURCE_SHA"],
                "${{ steps.release.outputs.source }}"
            );
            let checkout = steps
                .iter()
                .position(|step| {
                    step["run"]
                        .as_str()
                        .is_some_and(|run| run.contains("git checkout --detach"))
                })
                .unwrap();
            let check_local = steps
                .iter()
                .position(|step| {
                    step["run"]
                        .as_str()
                        .is_some_and(|run| run.contains("module_release check-local"))
                })
                .unwrap();
            assert!(checkout < validation && validation < check_local);
            let node = steps
                .iter()
                .find(|step| step["uses"] == "actions/setup-node@v7")
                .unwrap();
            assert_eq!(node["with"]["node-version"], "24");
            assert_eq!(node["with"]["package-manager-cache"], false);
        }
        let docs: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&fs::read_to_string(output.join("documentation.yml")).unwrap())
                .unwrap();
        let commands: Vec<_> = docs["jobs"]["build"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|step| step["run"].as_str())
            .collect();
        assert!(commands.contains(&if customized {
            "npm --prefix sites ci && npm --prefix sites run check"
        } else {
            "android-repo-setup build_docs"
        }));
        assert!(commands.contains(&if customized {
            "node sites/scripts/screenshots.mjs manifest"
        } else {
            "cd docs/screenshots && sha256sum -c images.sha256"
        }));
    }
}

#[test]
fn zola_sites_install_verified_rust_renderer_without_node() {
    let root = fixture();
    fs::create_dir_all(root.path().join("sites")).unwrap();
    fs::write(
        root.path().join("sites/zola.toml"),
        "base_url = 'https://example.invalid/demo'\n",
    )
    .unwrap();
    let result = run(root.path(), &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for name in [
        "documentation.yml",
        "publish-library.yml",
        "publish-jitpack.yml",
    ] {
        let text = fs::read_to_string(root.path().join(".github/workflows").join(name)).unwrap();
        assert!(
            !text.contains("actions/setup-node"),
            "{name} should not require Node"
        );
        let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).unwrap();
        let steps: Vec<_> = workflow["jobs"]
            .as_mapping()
            .unwrap()
            .values()
            .filter_map(|job| job["steps"].as_sequence())
            .flatten()
            .collect();
        let install = steps
            .iter()
            .find(|s| s["name"] == "Install pinned Zola")
            .expect("Zola installation step");
        let script = install["run"].as_str().unwrap();
        assert!(script.contains("v0.23.6"));
        assert!(script.contains("8f5132b3522412d04e395e0b25f6d68613ad272a873e54a2b3ebf664873024a4"));
        assert!(script.find("sha256sum -c").unwrap() < script.find("tar -xzf").unwrap());
        if name != "documentation.yml" {
            assert_eq!(install["if"], "steps.release.outputs.skip != 'true'");
        }
    }
}

#[test]
fn discovered_modules_get_independent_workflows_and_disabled_modules_are_omitted() {
    let root = fixture();
    let path = root.path().join(".project/setup/config.yaml");
    let mut config: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["android_modules"] = serde_yaml_ng::from_str(
        r#"
libs--vision:
  path: ':libs:vision'
  directory: components/detection
  kind: library
  selected: true
model:
  path: ':model'
  directory: model
  kind: library
  selected: true
phone:
  path: ':apps:phone'
  directory: mobile/phone
  kind: application
  selected: true
unused:
  path: ':unused'
  directory: unused
  kind: application
  selected: false
"#,
    )
    .unwrap();
    fs::write(&path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    let result = run(root.path(), &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = root.path().join(".github/workflows");
    assert!(output.join("publish-libs--vision.yml").exists());
    assert!(output.join("publish-model.yml").exists());
    assert!(!output.join("publish-demo-common.yml").exists());
    assert!(!output.join("build-signed-unused-apk.yml").exists());
    let apk = fs::read_to_string(output.join("build-signed-phone-apk.yml")).unwrap();
    assert!(apk.contains(":apps:phone:assembleRelease"));
    assert!(apk.contains("mobile/phone/build/outputs/apk/release/"));
    let finalize = fs::read_to_string(output.join("finalize-release.yml")).unwrap();
    assert!(finalize.contains("options: [\"libs--vision\", \"model\"]"));
    assert!(!output.join("screenshots.yml").exists());
    let docs: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&fs::read(output.join("documentation.yml")).unwrap()).unwrap();
    assert!(docs["on"]["workflow_run"]["workflows"]
        .as_sequence()
        .unwrap()
        .contains(&"Publish model".into()));
    assert!(fs::read_to_string(output.join("update-workflows.yml"))
        .unwrap()
        .contains("git add .project/setup/modules.properties"));
    assert!(run(root.path(), &["--check"]).status.success());
    config["android_modules"]["model"]["selected"] = false.into();
    fs::write(&path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    assert!(!run(root.path(), &["--check"]).status.success());
    assert!(run(root.path(), &["--overwrite"]).status.success());
    assert!(!output.join("publish-model.yml").exists());
}

#[test]
fn app_only_and_library_only_generation_and_registry_updates() {
    for kind in ["application", "library"] {
        let root = fixture();
        let config_path = root.path().join(".project/setup/config.yaml");
        let mut config: serde_yaml_ng::Value =
            serde_yaml_ng::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["android_modules"] = serde_yaml_ng::from_str(&format!("only:\n  path: ':only'\n  directory: 'source/mobile app'\n  kind: {kind}\n  selected: true\n")).unwrap();
        fs::write(&config_path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
        fs::write(root.path().join(".project/setup/repositories.yml"), "repositories:\n  maven-central: {environment: central, publisher: central}\n  jitpack: {environment: jitpack, publisher: jitpack}\n").unwrap();
        let result = run(root.path(), &[]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = root.path().join(".github/workflows");
        assert_eq!(output.join("publish-only.yml").exists(), kind == "library");
        assert_eq!(
            output.join("build-signed-only-apk.yml").exists(),
            kind == "application"
        );
        let inventory =
            fs::read_to_string(root.path().join(".project/setup/modules.properties")).unwrap();
        assert!(inventory.contains("only.path=:only"));
        let generated = Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
            .arg("--project")
            .arg(root.path())
            .args(["publishing_config", "generate"])
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        if kind == "application" {
            let apk = fs::read_to_string(output.join("build-signed-only-apk.yml")).unwrap();
            assert!(apk.contains("apks=('source/mobile app'/build/outputs/apk/release/"));
        }
    }
}

#[test]
fn yaml_like_module_ids_remain_strings_and_discovered_projects_skip_demo_screenshots() {
    let root = fixture();
    let path = root.path().join(".project/setup/config.yaml");
    let mut config: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["android_modules"] = serde_yaml_ng::from_str(
        "'true': {path: ':true', directory: lib, kind: library, selected: true}",
    )
    .unwrap();
    fs::write(path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    let result = run(root.path(), &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = root.path().join(".github/workflows");
    let publish: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&fs::read(output.join("publish-true.yml")).unwrap()).unwrap();
    assert_eq!(
        publish["jobs"]["publish"]["with"]["module"].as_str(),
        Some("true")
    );
    let docs = fs::read_to_string(output.join("documentation.yml")).unwrap();
    assert!(!docs.contains("cd docs/screenshots"));
}
