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

fn canonical_record(provider: &str) -> Value {
    let source = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut record = json!({
        "module": "demo-common", "version": "1.9.3", "source": source,
        "group": "com.example.android", "artifact": format!("{provider}-artifact"),
        "phase": "confirmed-public", "repository": provider,
        "source_tag": format!("demo-common/v1.9.3-{provider}"),
        "release_identity": {"schema": 1, "module": "demo-common", "version": "1.9.3",
            "source": source, "fingerprint": "b".repeat(64)}
    });
    if provider == "jitpack" {
        record["consumer_version"] = json!("demo-common~v1.9.3-jitpack");
        record["repository_url"] = json!("https://jitpack.io");
    }
    record
}

#[test]
fn canonical_destinations_merge_only_with_the_same_embedded_identity() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("fixtures/config.yaml"),
    );
    let central = canonical_record("maven-central");
    write(root, "docs/releases/demo-common.json", &central.to_string());
    success(root, &["documentation_history", "seed"]);
    let path = root.join("docs/releases/history/demo-common/1.9.3.json");
    assert!(
        path.exists(),
        "Canonical versions use a shared version route"
    );
    let jitpack = canonical_record("jitpack");
    let original = fs::read(&path).unwrap();
    for field in ["fingerprint", "source"] {
        let mut conflict = jitpack.clone();
        if field == "source" {
            conflict["source"] = json!("cccccccccccccccccccccccccccccccccccccccc");
            conflict["release_identity"][field] = conflict["source"].clone();
        } else {
            conflict["release_identity"][field] = json!("c".repeat(64));
        }
        write(
            root,
            "docs/releases/jitpack/demo-common.json",
            &conflict.to_string(),
        );
        assert!(!run(root, &["documentation_history", "seed"])
            .status
            .success());
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "Conflicts must not rewrite history"
        );
    }
    write(
        root,
        "docs/releases/jitpack/demo-common.json",
        &jitpack.to_string(),
    );
    success(root, &["documentation_history", "seed"]);
    success(root, &["documentation_history", "seed"]);
    let output = success(root, &["documentation_history", "export"]);
    let catalog: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(catalog.as_array().unwrap().len(), 1);
    assert_eq!(catalog[0]["route_version"], "1.9.3");
    assert_eq!(catalog[0]["destinations"]["maven-central"], central);
    assert_eq!(catalog[0]["destinations"]["jitpack"], jitpack);
    let installation = catalog[0]["installation"]["en"].as_str().unwrap();
    assert!(installation.contains("com.example.android:maven-central-artifact:1.9.3"));
    assert!(
        installation.contains("com.example.android:jitpack-artifact:demo-common~v1.9.3-jitpack")
    );
}

#[test]
fn archive_rejects_malformed_embedded_identity_and_legacy_canonical_mixing() {
    for (field, invalid) in [
        ("schema", json!(2)),
        ("module", json!("other")),
        ("version", json!("1.9.4")),
        ("source", json!("c".repeat(40))),
        ("fingerprint", json!("b".repeat(63))),
        ("fingerprint", json!("G".repeat(64))),
    ] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        write(
            root,
            ".project/setup/config.yaml",
            include_str!("fixtures/config.yaml"),
        );
        let mut record = canonical_record("maven-central");
        record["release_identity"][field] = invalid;
        write(root, "docs/releases/demo-common.json", &record.to_string());
        assert!(
            !run(root, &["documentation_history", "seed"])
                .status
                .success(),
            "Invalid identity {field} accepted"
        );
    }
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("fixtures/config.yaml"),
    );
    let mut legacy = canonical_record("maven-central");
    legacy.as_object_mut().unwrap().remove("source_tag");
    legacy.as_object_mut().unwrap().remove("release_identity");
    write(root, "docs/releases/demo-common.json", &legacy.to_string());
    success(root, &["documentation_history", "seed"]);
    let path = root.join("docs/releases/history/demo-common/1.9.3.json");
    let original = fs::read(&path).unwrap();
    write(
        root,
        "docs/releases/jitpack/demo-common.json",
        &canonical_record("jitpack").to_string(),
    );
    assert!(!run(root, &["documentation_history", "seed"])
        .status
        .success());
    assert_eq!(fs::read(path).unwrap(), original);
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

    let missing_archive = run(root, &["documentation_history", "verify"]);
    assert!(!missing_archive.status.success());
    assert!(String::from_utf8_lossy(&missing_archive.stderr)
        .contains("android-repo-setup documentation_history seed"));
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
