use std::{fs, path::Path, process::Command};
fn write(root: &Path, path: &str, body: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, body).unwrap();
}
#[test]
fn shared_astro_reports_missing_node_before_attempting_build() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".project/docs/config.yaml", "schema: 1\nrepository: example/library\ntitle: Reference\ntooling_ref: v0.1.20\nmodules:\n  api: API\n");
    write(dir.path(), "docs/agents/index.md", "# Overview\n");
    write(dir.path(), "docs/releases.md", "# Releases\n");
    write(dir.path(), "IMPORT.md", "# Installation\n");
    let output = Command::new(env!("CARGO_BIN_EXE_android-repo-docs"))
        .arg("--project")
        .arg(dir.path())
        .arg("build")
        .env("PATH", dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Node.js"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn failed_astro_build_preserves_last_good_output() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, ".project/docs/config.yaml", "schema: 1\nrepository: example/library\ntitle: Reference\ntooling_ref: v0.1.20\nmodules:\n  api: API\n");
    write(root, "docs/agents/index.md", "# Overview\n");
    write(root, "docs/releases.md", "# Releases\n");
    write(root, "IMPORT.md", "# Installation\n");
    write(root, "sites/dist/index.html", "last good build");
    write(root, "bin/node", "#!/bin/sh\necho v24.22.1\n");
    write(root, "bin/npm", "#!/bin/sh\nif [ \"$1\" = run ]; then\n  echo broken > \"$ANDROID_REPO_DOCS_OUTPUT/index.html\"\n  exit 17\nfi\necho 11.0.0\n");
    for file in ["bin/node", "bin/npm"] {
        fs::set_permissions(root.join(file), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_android-repo-docs"))
        .arg("--project")
        .arg(root)
        .arg("build")
        .env("PATH", root.join("bin"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Astro build failed"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("sites/dist/index.html")).unwrap(),
        "last good build"
    );
}

#[test]
#[ignore = "requires Node.js 24, npm and dependency registry access"]
fn real_shared_astro_cli_renders_history_scopes_raw_routes_and_prefix_links() {
    use serde_json::{json, Value};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
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
    let cli = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_android-repo-docs"))
            .arg("--project")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    write(root, ".project/docs/config.yaml", "schema: 1\nrepository: example/library\ntitle: Shared reference\ntooling_ref: v0.1.20\nmodules:\n  api: Public API\nsite:\n  backend: astro\n  base_url: https://example.com/original/\n");
    write(
        root,
        "docs/agents/index.md",
        "# Released overview\n\n[Deep guide](nested/deep.md#usage)\n",
    );
    write(
        root,
        "docs/agents/nested/deep.md",
        "# Deep guide\n\n## Usage\n\nReleased behavior.\n",
    );
    write(root, "docs/releases.md", "# Publishing\n");
    write(root, "IMPORT.md", "# Installation\n");
    git(&["init"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.com",
        "commit",
        "-m",
        "source",
    ]);
    let source = git(&["rev-parse", "HEAD"]);
    let record = json!({"module":"api","version":"1.2.3","source":source,"group":"com.example","artifact":"released-api","phase":"confirmed-public","repository":"maven-central","source_tag":"api/v1.2.3-maven-central","release_identity":{"schema":1,"module":"api","version":"1.2.3","source":source,"fingerprint":"a".repeat(64)}});
    write(root, "docs/releases/api.json", &record.to_string());
    cli(&["history", "seed"]);
    write(
        root,
        "docs/agents/nested/deep.md",
        "# Deep guide\n\n## Usage\n\nDevelopment behavior.\n",
    );
    cli(&["build", "--base", "/reference/", "--check"]);
    let dist = root.join("sites/dist");
    let page = fs::read_to_string(dist.join("en/api/1.2.3/nested/deep/index.html")).unwrap();
    assert!(page.contains("Released behavior."));
    assert!(!page.contains("Development behavior."));
    assert!(page.contains("id=\"usage\""));
    assert!(page.contains("data-pagefind-filter=\"version:1.2.3\""));
    assert!(page.contains("/reference/"));
    let development =
        fs::read_to_string(dist.join("en/api/development/nested/deep/index.html")).unwrap();
    assert!(development.contains("Development behavior."));
    let overview = fs::read_to_string(dist.join("en/api/1.2.3/index.html")).unwrap();
    assert!(overview.contains("/reference/en/api/1.2.3/nested/deep/#usage"));
    let raw = fs::read_to_string(dist.join("en/api/1.2.3/raw/index.md")).unwrap();
    assert!(raw.contains(&source));
    assert!(raw.contains("/reference/en/api/1.2.3/raw/nested/deep.md#usage"));
    let spanish = fs::read_to_string(dist.join("es/api/1.2.3/index.html")).unwrap();
    assert!(spanish.contains("lang=\"es\""));
    assert!(dist.join("en/index.html").is_file());
    assert!(dist.join("es/index.html").is_file());
    assert!(dist.join("pagefind/pagefind.js").is_file());
    assert!(dist.join("1.2.3/en/index.html").is_file());
    assert!(dist.join("IMPORT.md").is_file());
    let scopes: Value =
        serde_json::from_slice(&fs::read(dist.join("versions.json")).unwrap()).unwrap();
    assert_eq!(scopes.as_array().unwrap().len(), 4);
    assert!(scopes
        .as_array()
        .unwrap()
        .iter()
        .any(|scope| scope["version"] == "1.2.3" && scope["source"] == source));
}
