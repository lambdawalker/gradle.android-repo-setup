use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
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

#[test]
fn installation_and_archive_use_confirmed_destination_coordinates() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        ".project/setup/config.yaml",
        &(include_str!("fixtures/config.yaml").to_owned() + "\nverification:\n  class_prefix: com/apexfission/android/permissions/\n  required_api_artifacts: [permissions]\n"),
    );
    write(
        root,
        "gradle.properties",
        "GROUP=com.example.android\nPOM_ARTIFACT_ID=next\n",
    );
    write(
        root,
        "docs/templates/MODULE_IMPORT.md.template",
        "# Fixture\n\n{{INSTALLATION}}\n",
    );
    let source = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let record = json!({
        "module": "demo-common", "version": "1.9.3", "source": source,
        "group": "com.example.android", "artifact": "current", "phase": "confirmed-public",
        "repository": "maven-central", "source_tag": "demo-common/v1.9.3-maven-central"
    });
    write(root, "docs/releases/demo-common.json", &record.to_string());

    success(root, &["module_release", "generate"]);
    let import = fs::read_to_string(root.join("IMPORT.md")).unwrap();
    assert!(import.contains("implementation(\"com.example.android:current:1.9.3\")"));
    assert!(import.contains("Configured next publication: `com.example.android:next`"));
    success(root, &["import_docs", "--check"]);

    assert!(!run(root, &["documentation_history", "verify"])
        .status
        .success());
    success(root, &["documentation_history", "seed"]);
    let output = success(root, &["documentation_history", "export"]);
    let catalog: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(catalog[0]["route_version"], "1.9.3-maven-central");
    assert_eq!(catalog[0]["documentation_ref"], source);
    assert!(catalog[0]["installation"]["es"]
        .as_str()
        .unwrap()
        .contains("Catálogo de versiones"));
}

#[test]
fn site_build_links_version_routes_and_loads_their_sources_from_git() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(&["init"]);
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("fixtures/config.yaml"),
    );
    for page in ["index", "quickstart", "api"] {
        write(
            root,
            &format!("docs/agents/{page}.md"),
            "# Released guide\n\n[Quickstart](quickstart.md#usage)\n",
        );
    }
    write(root, "docs/releases.md", "# Publishing guide\n");
    write(root, "sites/style.css", "main {}");
    write(root, "docs/screenshots/catalog-light.png", "released light");
    write(root, "docs/screenshots/catalog-dark.png", "released dark");
    write(root, "IMPORT.md", "# Development installation\n");
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.com",
        "commit",
        "-m",
        "fixture",
    ]);
    let source = git(&["rev-parse", "HEAD"]);
    let record = json!({
        "module": "demo-common", "version": "1.9.3", "source": source,
        "group": "com.example.android", "artifact": "current", "phase": "confirmed-public"
    });
    write(root, "docs/releases/demo-common.json", &record.to_string());
    write(root, "docs/agents/index.md", "# Development guide\n");
    write(
        root,
        "docs/es/index.md",
        "# Guía rápida\n\n| API |\n| --- |\n| Demo |\n\n```kotlin\nval title = \"Demo\"\n```\n",
    );
    write(
        root,
        "docs/screenshots/catalog-light.png",
        "development light",
    );

    success(root, &["documentation_history", "seed"]);
    success(root, &["build_docs", "--base", "/reference/", "--check"]);
    let dist = root.join("sites/dist");
    let index = fs::read_to_string(dist.join("index.html")).unwrap();
    assert!(index.contains("/reference/1.9.3/en/index.html"));
    let historical = fs::read_to_string(dist.join("1.9.3/es/index.html")).unwrap();
    assert!(historical.contains("<h1 id=\"released-guide\">Released guide</h1>"));
    assert!(historical.contains("Guía en inglés para esta versión."));
    assert!(historical.contains("href=\"quickstart.html#usage\""));
    let development = fs::read_to_string(dist.join("development/es/index.html")).unwrap();
    assert!(development.contains("<h1 id=\"guia-rapida\">Guía rápida</h1>"));
    assert!(development.contains("<table>\n<thead>\n<tr>\n<th>API</th>"));
    assert!(development.contains("val title = &quot;Demo&quot;"));
    assert_eq!(
        fs::read_to_string(dist.join("media/1.9.3/catalog-light.png")).unwrap(),
        "released light"
    );
    assert_eq!(
        fs::read(dist.join("IMPORT.md")).unwrap(),
        fs::read(dist.join("1.9.3/en/IMPORT.md")).unwrap()
    );
}
