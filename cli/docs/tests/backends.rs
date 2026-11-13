use android_repo_docs::{site_backend, SiteBackend};
use std::fs;
fn project(site: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".project/docs")).unwrap();
    fs::write(root.path().join(".project/docs/config.yaml"), format!("schema: 1\nrepository: example/library\ntitle: Library\ntooling_ref: v0.1.20\nmodules: {{}}\n{site}")).unwrap();
    root
}
#[test]
fn fresh_sites_default_to_astro_and_existing_zola_sites_keep_their_renderer() {
    let root = project("");
    assert_eq!(site_backend(root.path()).unwrap(), SiteBackend::Astro);
    fs::create_dir_all(root.path().join("sites")).unwrap();
    fs::write(
        root.path().join("sites/zola.toml"),
        "base_url = 'https://example.com/'",
    )
    .unwrap();
    assert_eq!(site_backend(root.path()).unwrap(), SiteBackend::Zola);
}
#[test]
fn explicit_backend_overrides_detection_and_external_commands_still_win_implicitly() {
    let root = project("site:\n  backend: astro\n");
    fs::create_dir_all(root.path().join("sites")).unwrap();
    fs::write(root.path().join("sites/zola.toml"), "").unwrap();
    assert_eq!(site_backend(root.path()).unwrap(), SiteBackend::Astro);
    let external = project("site:\n  build: [npm, run, build]\n");
    assert_eq!(
        site_backend(external.path()).unwrap(),
        SiteBackend::External
    );
    let legacy = project("site:\n  backend: legacy\n");
    assert_eq!(site_backend(legacy.path()).unwrap(), SiteBackend::Legacy);
}
#[test]
fn invalid_and_ambiguous_backend_configuration_is_rejected() {
    for site in [
        "site:\n  backend: typo\n",
        "site:\n  backend: astro\n  build: [npm, run, build]\n",
        "site:\n  backend: external\n",
        "site:\n  typo: true\n",
    ] {
        assert!(site_backend(project(site).path()).is_err(), "{site}");
    }
}
#[test]
fn fresh_initialization_records_astro_default_without_rewriting_existing_config() {
    let root = tempfile::tempdir().unwrap();
    android_repo_docs::init_with(
        root.path(),
        Some("example/library"),
        Some("Library"),
        Some("v0.1.20"),
        &[],
    )
    .unwrap();
    let path = root.path().join(".project/docs/config.yaml");
    let original = fs::read_to_string(&path).unwrap();
    assert!(original.contains("backend: astro"));
    let metadata: serde_json::Value = serde_yaml_ng::from_str(&original).unwrap();
    assert_eq!(
        metadata["modules"]["library"], "Library",
        "Fresh docs-only projects must have a renderable module"
    );
    android_repo_docs::init(root.path()).unwrap();
    assert_eq!(original, fs::read_to_string(path).unwrap());
}

#[test]
#[ignore = "requires Node.js 24, npm and dependency registry access"]
fn fresh_docs_only_repository_builds_with_inferred_module_at_domain_root() {
    use std::process::Command;
    let root = tempfile::tempdir().unwrap();
    android_repo_docs::init_with(
        root.path(),
        Some("example/library"),
        Some("Library"),
        Some("v0.1.20"),
        &[],
    )
    .unwrap();
    fs::create_dir_all(root.path().join("docs/agents")).unwrap();
    fs::write(
        root.path().join("docs/agents/index.md"),
        "# Overview\n\nA fresh documentation-only site.\n",
    )
    .unwrap();
    fs::write(root.path().join("docs/releases.md"), "# Publishing\n").unwrap();
    fs::write(
        root.path().join("IMPORT.md"),
        "# Installation\n\nNo confirmed release.\n",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_android-repo-docs"))
        .arg("--project")
        .arg(root.path())
        .args(["build", "--base", "/", "--check"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let page = fs::read_to_string(
        root.path()
            .join("sites/dist/en/library/development/index.html"),
    )
    .unwrap();
    assert!(page.contains("A fresh documentation-only site."));
    assert!(root
        .path()
        .join("sites/dist/en/library/development/raw/index.md")
        .is_file());
    assert!(!root.path().join(".project/setup").exists());
}
