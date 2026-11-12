use std::{fs, path::Path, process::Command};
fn write(root: &Path, path: &str, text: &str) {
    let p = root.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}
fn run(root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
        .args(["--project"])
        .arg(root)
        .args(["build_docs", "--base", "/reference/", "--check"])
        .output()
        .unwrap()
}
#[test]
fn real_zola_builds_recursive_bilingual_scopes_and_raw_routes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("fixtures/config.yaml"),
    );
    write(root,"sites/zola.toml","base_url = \"https://example.com/original/\"\ndefault_language = \"en\"\nbuild_search_index = true\n[search]\nindex_format = \"fuse_json\"\n[languages.es]\nbuild_search_index = true\n");
    write(
        root,
        "docs/agents/index.md",
        "# Overview\n\n[Deep][deep]\n\n[deep]: nested/deep.md#usage\n\n`[Code](nested/deep.md)`\n\n```markdown\n# Fence heading\n[Code](nested/deep.md)\n```\n",
    );
    write(
        root,
        "docs/agents/nested/deep.md",
        "# Deep guide\n\n## Usage\n\nHello world.\n",
    );
    write(root, "docs/releases.md", "# Publishing\n");
    write(root, "IMPORT.md", "Generated preamble.\n\n# Installation\n");
    let out = run(root);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dist = root.join("sites/dist");
    assert!(dist
        .join("en/demo-common/development/nested/deep/index.html")
        .exists());
    assert!(dist.join("es/demo-common/development/index.html").exists());
    let scopes: serde_json::Value =
        serde_json::from_slice(&fs::read(dist.join("versions.json")).unwrap()).unwrap();
    assert_eq!(scopes[0]["root"], "/reference/en/demo-common/development/");
    assert!(dist
        .join("en/demo-common/development/raw/nested/deep.md")
        .exists());
    let raw = fs::read_to_string(dist.join("en/demo-common/development/raw/index.md")).unwrap();
    assert!(raw.contains("`[Code](nested/deep.md)`"));
    assert!(raw.contains("```markdown\n# Fence heading\n[Code](nested/deep.md)\n```"));
    assert!(raw.contains("[Deep](/reference/en/demo-common/development/raw/nested/deep.md#usage)"));
    assert!(dist.join("development/en/index.html").exists());
    assert!(dist.join("search_index.en.json").exists());
    assert!(dist.join("search_index.es.json").exists());
    assert!(fs::read_to_string(dist.join("search_index.en.json"))
        .unwrap()
        .contains("https://example.com/reference/en/"));
    assert!(
        fs::read_to_string(dist.join("en/demo-common/development/index.html"))
            .unwrap()
            .contains("/reference/en/demo-common/development/nested/deep/#usage")
    );
}

#[test]
fn real_zola_merges_canonical_destinations_in_one_bilingual_version_scope() {
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
    git(&["init"]);
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("fixtures/config.yaml"),
    );
    write(
        root,
        "sites/zola.toml",
        "base_url = \"https://example.com/\"\n",
    );
    write(root, "docs/agents/index.md", "# Released overview\n");
    write(root, "docs/releases.md", "# Publishing\n");
    write(root, "IMPORT.md", "# Development installation\n");
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
    for provider in ["maven-central", "jitpack"] {
        let mut record = json!({"module":"demo-common","version":"1.9.3","source":source,
            "group":"com.example.android","artifact":format!("{provider}-artifact"),
            "phase":"confirmed-public","repository":provider,"source_tag":format!("demo-common/v1.9.3-{provider}"),
            "release_identity":{"schema":1,"module":"demo-common","version":"1.9.3","source":source,"fingerprint":"a".repeat(64)}});
        if provider == "jitpack" {
            record["consumer_version"] = json!("demo-common~v1.9.3-jitpack");
            record["repository_url"] = json!("https://jitpack.io");
        }
        let path = if provider == "maven-central" {
            "docs/releases/demo-common.json"
        } else {
            "docs/releases/jitpack/demo-common.json"
        };
        write(root, path, &record.to_string());
        let seed = Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
            .arg("--project")
            .arg(root)
            .args(["documentation_history", "seed"])
            .output()
            .unwrap();
        assert!(
            seed.status.success(),
            "{}",
            String::from_utf8_lossy(&seed.stderr)
        );
    }
    let out = run(root);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dist = root.join("sites/dist");
    let scopes: Value =
        serde_json::from_slice(&fs::read(dist.join("versions.json")).unwrap()).unwrap();
    let released: Vec<_> = scopes
        .as_array()
        .unwrap()
        .iter()
        .filter(|scope| scope["development"] == false)
        .collect();
    assert_eq!(
        released.len(),
        2,
        "One canonical release scope per language"
    );
    for lang in ["en", "es"] {
        let scope = released
            .iter()
            .find(|scope| scope["language"] == lang)
            .unwrap();
        assert_eq!(scope["version"], "1.9.3");
        assert_eq!(scope["source"], source);
        assert_eq!(scope["destinations"], json!(["jitpack", "maven-central"]));
        let installation = fs::read_to_string(
            dist.join(format!("{lang}/demo-common/1.9.3/installation/index.html")),
        )
        .unwrap();
        assert!(installation.contains("com.example.android:maven-central-artifact:1.9.3"));
        assert!(installation
            .contains("com.example.android:jitpack-artifact:demo-common~v1.9.3-jitpack"));
        assert!(dist.join(format!("1.9.3/{lang}/install.html")).exists());
        assert!(!dist
            .join(format!("{lang}/demo-common/1.9.3-jitpack"))
            .exists());
    }
}

#[test]
fn real_zola_keeps_provider_archives_and_fresh_translation_overlays() {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap().trim().to_owned()
    };
    git(&["init"]);
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("fixtures/config.yaml"),
    );
    write(
        root,
        "sites/zola.toml",
        "base_url = \"https://example.com/\"\n",
    );
    let english="# Overview\n\n## Usage\n\n[Install](../../IMPORT.md)\n\n[Code](../../src/Sample.kt)\n\n![Picture](../screenshots/pinned.svg)\n";
    write(root, "docs/agents/index.md", english);
    write(
        root,
        "docs/screenshots/pinned.svg",
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><text>released</text></svg>",
    );
    write(root, "docs/releases.md", "# Publishing\n");
    write(root, "IMPORT.md", "# Development installation\n");
    write(root, "src/Sample.kt", "// first provider source\n");
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
    write(
        root,
        "docs/es/index.md",
        "# Resumen\n\n## Uso\n\nTraducción fresca.\n",
    );
    write(
        root,
        "docs/es/translations.json",
        &json!({"index.md":format!("{:x}",Sha256::digest(english.as_bytes()))}).to_string(),
    );
    git(&["add", "docs/es"]);
    let root_tree = git(&["write-tree"]);
    let tree = git(&["rev-parse", &format!("{root_tree}:docs/es")]);
    write(root, "src/Sample.kt", "// second provider source\n");
    git(&["add", "src/Sample.kt"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.com",
        "commit",
        "-m",
        "second provider source",
    ]);
    let second_source = git(&["rev-parse", "HEAD"]);
    for (provider, artifact) in [
        ("maven-central", "central-artifact"),
        ("jitpack", "jitpack-artifact"),
    ] {
        let provider_source = if provider == "jitpack" {
            &second_source
        } else {
            &source
        };
        let mut record = json!({"module":"demo-common","version":"1.9.3","source":provider_source,"group":"com.example.android","artifact":artifact,"phase":"confirmed-public","repository":provider,"source_tag":format!("demo-common/v1.9.3-{provider}")});
        if provider == "jitpack" {
            record["consumer_version"] = json!("demo-common~v1.9.3-jitpack");
            record["repository_url"] = json!("https://jitpack.io");
        }
        let entry = json!({"schema":1,"module":"demo-common","version":"1.9.3","source":provider_source,"documentation_ref":source,"translation_tree":tree,"destinations":{provider:record}});
        write(
            root,
            &format!("docs/releases/history/demo-common/1.9.3-{provider}.json"),
            &entry.to_string(),
        );
    }
    write(
        root,
        "docs/agents/index.md",
        "# Changed development\n\n## Changed\n",
    );
    write(root, "docs/screenshots/pinned.svg", "development picture");
    let out = run(root);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dist = root.join("sites/dist");
    let es =
        fs::read_to_string(dist.join("es/demo-common/1.9.3-maven-central/index.html")).unwrap();
    assert!(es.contains("Traducción fresca"));
    assert!(!es.contains("Resumen {#"));
    let raw =
        fs::read_to_string(dist.join("es/demo-common/1.9.3-maven-central/raw/index.md")).unwrap();
    assert!(raw.contains("Source:"));
    assert!(raw.contains("<a id=\"usage\"></a>"));
    assert!(!raw.contains("{#usage}"));
    assert!(es.contains("id=\"usage\""));
    let dev = fs::read_to_string(dist.join("es/demo-common/development/index.html")).unwrap();
    assert!(dev.contains("Changed development"));
    assert!(!dev.contains("Traducción fresca"));
    let central =
        fs::read_to_string(dist.join("en/demo-common/1.9.3-maven-central/installation/index.html"))
            .unwrap();
    assert!(central.contains("central-artifact"));
    assert!(!central.contains("jitpack-artifact"));
    let jp = fs::read_to_string(dist.join("en/demo-common/1.9.3-jitpack/installation/index.html"))
        .unwrap();
    assert!(jp.contains("jitpack-artifact"));
    assert!(jp.contains("demo-common~v1.9.3-jitpack"));
    assert!(!jp.contains("central-artifact"));
    let jp_overview =
        fs::read_to_string(dist.join("en/demo-common/1.9.3-jitpack/index.html")).unwrap();
    assert!(jp_overview.contains(&format!("/blob/{second_source}/src/Sample.kt")));
    let image = fs::read_to_string(
        dist.join("en/demo-common/1.9.3-maven-central/media/docs/screenshots/pinned.svg"),
    )
    .unwrap();
    assert!(image.contains("released"));
    assert!(dist.join("1.9.3-jitpack/en/install.html").exists());
    assert!(dist.join("1.9.3-maven-central/es/releases.html").exists());
    let archive_path = root.join("docs/releases/history/demo-common/1.9.3-maven-central.json");
    let mut archive: serde_json::Value =
        serde_json::from_slice(&fs::read(&archive_path).unwrap()).unwrap();
    archive["translation_tree"] = json!("ffffffffffffffffffffffffffffffffffffffff");
    fs::write(archive_path, archive.to_string()).unwrap();
    let unavailable = run(root);
    assert!(
        !unavailable.status.success(),
        "Unavailable historical translation objects must fail the build"
    );
    assert!(String::from_utf8_lossy(&unavailable.stderr).contains("fetch"));
}

#[test]
fn check_rejects_broken_local_anchors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("fixtures/config.yaml"),
    );
    write(
        root,
        "sites/zola.toml",
        "base_url = \"https://example.com/\"\n",
    );
    write(
        root,
        "docs/agents/index.md",
        "# Overview\n\n[Missing](#does-not-exist)\n",
    );
    write(root, "IMPORT.md", "# Install\n");
    write(root, "docs/releases.md", "# Publishing\n");
    let out = run(root);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("does-not-exist"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn check_rejects_missing_local_images() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("fixtures/config.yaml"),
    );
    write(
        root,
        "sites/zola.toml",
        "base_url = \"https://example.com/\"\n",
    );
    write(
        root,
        "docs/agents/index.md",
        "# Overview\n\n![Missing](../screenshots/absent.png)\n",
    );
    write(root, "IMPORT.md", "# Installation\n");
    write(root, "docs/releases.md", "# Publishing\n");
    let out = run(root);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Missing documentation image"));
}

#[test]
fn distinct_recursive_names_do_not_collide_in_staged_content() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("fixtures/config.yaml"),
    );
    write(
        root,
        "sites/zola.toml",
        "base_url = \"https://example.com/\"\n",
    );
    write(root, "docs/agents/index.md", "# Overview\n");
    write(
        root,
        "docs/agents/nested/a.md",
        "# Nested page\n\nNested content.\n",
    );
    write(
        root,
        "docs/agents/nested-a.md",
        "# Flat page\n\nFlat content.\n",
    );
    write(root, "IMPORT.md", "# Installation\n");
    write(root, "docs/releases.md", "# Publishing\n");
    let out = run(root);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dist = root.join("sites/dist");
    for lang in ["en", "es"] {
        let nested = fs::read_to_string(dist.join(format!(
            "{lang}/demo-common/development/nested/a/index.html"
        )))
        .unwrap();
        let flat = fs::read_to_string(dist.join(format!(
            "{lang}/demo-common/development/nested-a/index.html"
        )))
        .unwrap();
        assert!(nested.contains("Nested content."));
        assert!(flat.contains("Flat content."));
    }
}

#[test]
fn linked_images_rewrite_both_destinations_without_overlapping_edits() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".project/setup/config.yaml",
        include_str!("fixtures/config.yaml"),
    );
    write(
        root,
        "sites/zola.toml",
        "base_url = \"https://example.com/\"\n",
    );
    write(root,"docs/agents/index.md","# Overview\n\n[![Picture](../screenshots/a.svg)](next.md)\n\n[![Reference picture][picture]][next]\n\n[picture]: ../screenshots/a.svg\n[next]: next.md\n");
    write(root, "docs/agents/next.md", "# Next\n");
    write(
        root,
        "docs/screenshots/a.svg",
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    );
    write(root, "IMPORT.md", "# Installation\n");
    write(root, "docs/releases.md", "# Publishing\n");
    let out = run(root);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dist = root.join("sites/dist");
    let html = fs::read_to_string(dist.join("en/demo-common/development/index.html")).unwrap();
    assert!(html.contains("href=\"/reference/en/demo-common/development/next/\""));
    assert_eq!(
        html.matches("src=\"/reference/en/demo-common/development/media/docs/screenshots/a.svg\"")
            .count(),
        2
    );
    let raw = fs::read_to_string(dist.join("en/demo-common/development/raw/index.md")).unwrap();
    assert!(raw.contains("[![Picture](/reference/en/demo-common/development/media/docs/screenshots/a.svg)](/reference/en/demo-common/development/raw/next.md)"));
    assert!(raw.contains("[![Reference picture](/reference/en/demo-common/development/media/docs/screenshots/a.svg)](/reference/en/demo-common/development/raw/next.md)"));
}
