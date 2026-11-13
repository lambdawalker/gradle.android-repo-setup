use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn write(root: &Path, path: &str, value: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}
fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_android-repo-docs"))
        .arg("--project")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}
fn success(root: &Path, args: &[&str]) -> Output {
    let output = run(root, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
fn config(root: &Path) {
    write(root, ".project/docs/config.yaml", "schema: 1\nrepository: example/library\ntitle: Independent documentation\ntooling_ref: v0.1.15\nmodules:\n  api: Public API\n");
    write(
        root,
        "gradle.properties",
        "GROUP=com.example\nPOM_ARTIFACT_ID=next-artifact\nPOM_ARTIFACT_ID.api=next-api\n",
    );
    write(
        root,
        "docs/templates/MODULE_IMPORT.md.template",
        "# Install\n\n{{INSTALLATION}}\n",
    );
}
fn confirmed(root: &Path) -> Value {
    let record = json!({"module": "api", "version": "1.2.3", "source": "a".repeat(40),
        "group": "com.example", "artifact": "released-api", "phase": "confirmed-public",
        "repository": "maven-central", "source_tag": "api/v1.2.3-maven-central"});
    write(root, "docs/releases/api.json", &record.to_string());
    record
}

#[test]
fn imports_use_independent_metadata_and_confirmed_coordinates() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    config(root);
    confirmed(root);
    success(root, &["imports"]);
    let import = fs::read_to_string(root.join("IMPORT.md")).unwrap();
    assert!(import.contains("com.example:released-api:1.2.3"));
    assert!(import.contains("com.example:next-api"));
    success(root, &["imports", "--check"]);
    write(root, "IMPORT.md", "manual drift\n");
    assert!(!run(root, &["imports", "--check"]).status.success());
    assert_eq!(
        fs::read_to_string(root.join("IMPORT.md")).unwrap(),
        "manual drift\n"
    );
}

#[test]
fn history_preserves_confirmed_records_and_rejects_conflicts_without_setup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    config(root);
    let mut record = confirmed(root);
    success(root, &["history", "seed"]);
    let path = root.join("docs/releases/history/api/1.2.3-maven-central.json");
    let original = fs::read(&path).unwrap();
    success(root, &["history", "verify"]);
    let export: Value =
        serde_json::from_slice(&success(root, &["history", "export"]).stdout).unwrap();
    assert_eq!(export[0]["destinations"]["maven-central"], record);
    assert!(export[0]["installation"]["es"]
        .as_str()
        .unwrap()
        .contains("Instalación"));
    record["source"] = json!("b".repeat(40));
    write(root, "docs/releases/api.json", &record.to_string());
    assert!(!run(root, &["history", "seed"]).status.success());
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn check_is_read_only_and_update_does_not_confirm_or_seed_history() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    config(root);
    confirmed(root);
    assert!(!run(root, &["check"]).status.success());
    assert!(!root.join("IMPORT.md").exists());
    assert!(!root.join("docs/releases/history").exists());
    success(root, &["update"]);
    assert!(root.join("IMPORT.md").exists());
    assert!(!root.join("docs/releases/history").exists());
    success(root, &["history", "seed"]);
    let before = fs::read(root.join("IMPORT.md")).unwrap();
    success(root, &["check"]);
    assert_eq!(fs::read(root.join("IMPORT.md")).unwrap(), before);
}

#[test]
fn init_copies_legacy_metadata_and_never_replaces_an_existing_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("../../tests/fixtures/config.yaml"),
    );
    success(root, &["init"]);
    let path = root.join(".project/docs/config.yaml");
    let original = fs::read(&path).unwrap();
    let config: Value = serde_yaml_ng::from_slice(&original).unwrap();
    assert_eq!(config["schema"], 1);
    assert_eq!(
        config["repository"],
        "lambdawalker/android.apexfission.demo-common"
    );
    assert!(config.get("signing").is_none());
    success(root, &["init"]);
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn independent_metadata_wins_and_invalid_schema_does_not_fall_back() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    config(root);
    write(
        root,
        ".project/setup/config.yaml",
        "invalid: [legacy metadata\n",
    );
    success(root, &["imports"]);
    assert!(fs::read_to_string(root.join("IMPORT.md"))
        .unwrap()
        .contains("Public API"));
    write(root, ".project/docs/config.yaml", "schema: 2\nrepository: example/library\ntitle: Invalid schema\ntooling_ref: v0.1.15\nmodules: {}\n");
    assert!(!run(root, &["imports"]).status.success());
}

#[test]
fn legacy_setup_metadata_remains_supported() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("../../tests/fixtures/config.yaml"),
    );
    write(root, "gradle.properties", "GROUP=com.example\n");
    write(
        root,
        "docs/templates/MODULE_IMPORT.md.template",
        "{{INSTALLATION}}\n",
    );
    success(root, &["imports"]);
    assert!(fs::read_to_string(root.join("IMPORT.md"))
        .unwrap()
        .contains("## Demo Common\n\nNo confirmed release."));
}

#[test]
fn zola_build_renders_embedded_theme_without_setup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    config(root);
    write(
        root,
        "sites/zola.toml",
        "base_url = \"https://example.com/\"\n",
    );
    write(
        root,
        "docs/agents/index.md",
        "# API overview\n\nDocumented behavior.\n",
    );
    write(root, "docs/releases.md", "# Publishing\n");
    success(root, &["imports"]);
    success(root, &["build", "--base", "/reference", "--check"]);
    let page = fs::read_to_string(root.join("sites/dist/en/api/development/index.html")).unwrap();
    assert!(page.contains("Documented behavior."));
    assert!(page.contains("/reference/"));
    assert!(root.join("sites/dist/docs.css").is_file());
}

#[test]
fn configured_site_commands_are_literal_argv_and_propagate_errors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    config(root);
    let original = fs::read_to_string(root.join(".project/docs/config.yaml")).unwrap();
    // Git is a portable external program with an observable output file. A single
    // argument containing shell metacharacters must remain literal.
    write(root, ".project/docs/config.yaml", &(original.clone() + "site:\n  build: [git, config, --file, site-output, docs.message, 'literal ; value']\n  serve: [git, config, --file, site-output, docs.served, yes]\n"));
    success(root, &["build"]);
    assert!(fs::read_to_string(root.join("site-output"))
        .unwrap()
        .contains("literal ; value"));
    success(root, &["serve"]);
    assert!(fs::read_to_string(root.join("site-output"))
        .unwrap()
        .contains("served = yes"));
    write(
        root,
        ".project/docs/config.yaml",
        &(original + "site:\n  build: [git, definitely-not-a-command]\n"),
    );
    assert!(!run(root, &["build"]).status.success());
}

#[test]
fn recursive_configured_build_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    config(root);
    let original = fs::read_to_string(root.join(".project/docs/config.yaml")).unwrap();
    let argv = json!([env!("CARGO_BIN_EXE_android-repo-docs"), "build"]);
    write(
        root,
        ".project/docs/config.yaml",
        &(original + &format!("site:\n  build: {argv}\n")),
    );
    let result = run(root, &["build"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Recursive"));
}

#[cfg(windows)]
#[test]
fn configured_site_resolves_path_only_npm_cmd_and_preserves_argv_and_environment() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    config(root);
    let original = fs::read_to_string(root.join(".project/docs/config.yaml")).unwrap();
    write(root, ".project/docs/config.yaml", &(original + "site:\n  build: [npm, 'first argument', 'literal & value', build]\n  serve: [npm, 'first argument', 'literal & value', fail]\n"));
    let shim_dir = root.join("node tools");
    write(root, "node tools/npm.cmd", "@echo off\r\nif not \"%~1\"==\"first argument\" exit /b 41\r\nif not \"%~2\"==\"literal & value\" exit /b 42\r\nif not \"%ANDROID_REPO_DOCS_SITE_ACTIVE%\"==\"1\" exit /b 43\r\nif \"%~3\"==\"fail\" (\r\n  if not \"%ANDROID_REPO_DOCS_PORT%\"==\"8123\" exit /b 44\r\n  exit /b 23\r\n)\r\nif not \"%ANDROID_REPO_DOCS_BASE%\"==\"/with space\" exit /b 45\r\nif not \"%ANDROID_REPO_DOCS_CHECK%\"==\"1\" exit /b 46\r\n>observed.txt echo shim executed\r\nexit /b 0\r\n");
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_android-repo-docs"))
            .arg("--project")
            .arg(root)
            .args(args)
            // No npm.exe, npm.cmd in a space-containing PATH directory only.
            .env("PATH", &shim_dir)
            .output()
            .unwrap()
    };
    let output = invoke(&["build", "--base", "/with space", "--check"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("observed.txt"))
            .unwrap()
            .trim(),
        "shim executed"
    );
    let output = invoke(&["serve", "--port", "8123"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("23"));
}

#[test]
fn source_only_project_checks_do_not_require_import_or_release_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".project/docs/config.yaml",
        "schema: 1\nrepository: example/library\ntitle: Sources\ntooling_ref: v0.1.15\n",
    );
    success(root, &["check"]);
    success(root, &["update"]);
    assert!(!root.join("IMPORT.md").exists());
    assert!(!root.join("docs/releases").exists());
}

#[test]
fn legacy_import_template_name_is_supported() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    config(root);
    fs::rename(
        root.join("docs/templates/MODULE_IMPORT.md.template"),
        root.join("docs/templates/IMPORT.md.template"),
    )
    .unwrap();
    success(root, &["update"]);
    assert!(fs::read_to_string(root.join("IMPORT.md"))
        .unwrap()
        .contains("Public API"));
    success(root, &["check"]);
}

#[test]
fn local_preview_serves_generated_pages_and_rejects_parent_paths() {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    config(root);
    write(
        root,
        "sites/zola.toml",
        "base_url = \"https://example.com/\"\n",
    );
    write(
        root,
        "docs/agents/index.md",
        "# Preview\n\nServed locally.\n",
    );
    write(root, "docs/releases.md", "# Publishing\n");
    write(root, "private.txt", "must not be served");
    success(root, &["imports"]);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut child = Child(
        Command::new(env!("CARGO_BIN_EXE_android-repo-docs"))
            .arg("--project")
            .arg(root)
            .args(["serve", "--port", &port.to_string(), "--base", "/preview"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "Preview exited before serving"
        );
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "Preview never started"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let request = |path: &str| {
        let mut connection = TcpStream::connect(("127.0.0.1", port)).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(
            connection,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        connection.read_to_string(&mut response).unwrap();
        response
    };
    let response = request("/preview/en/api/development/?preview=yes");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("Served locally."));
    for path in [
        "/../../private.txt",
        "/%2e%2e/%2e%2e/private.txt",
        "/%5c..%5cprivate.txt",
    ] {
        let response = request(path);
        assert!(!response.starts_with("HTTP/1.1 200"));
        assert!(!response.contains("must not be served"));
    }
}
