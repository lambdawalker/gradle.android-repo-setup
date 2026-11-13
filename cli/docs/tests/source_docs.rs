use android_repo_docs::sources;
use std::fs;
#[test]
fn sources_roundtrip_and_implementation_hash() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::create_dir_all(r.join(".project/docs")).unwrap();
    fs::create_dir(r.join("src")).unwrap();
    fs::write(r.join(".project/docs/config.yaml"),"schema: 1\nrepository: test/sample\ntitle: Sample\ntooling_ref: v0.1.15\nsources:\n  roots: [src]\n  manifest: manifest.json\n  groups:\n    - include: ['**/*.kt']\n      output: api.md\n").unwrap();
    fs::write(r.join("src/A.kt"),"class Public(val x: Int = 2) {\n private class Hidden\n fun run(x: Int): Int { return x + 1 }\n fun run(): Int = 2\n}\n").unwrap();
    sources::update(r).unwrap();
    sources::check(r).unwrap();
    let api = fs::read_to_string(r.join("api.md")).unwrap();
    assert!(api.contains("class Public"));
    assert!(api.contains("fun run()"));
    assert!(!api.contains("Hidden"));
    assert!(!api.contains("return x"));
    fs::write(r.join("src/A.kt"),"class Public(val x: Int = 2) {\n private class Hidden\n fun run(x: Int): Int { return x + 9 }\n fun run(): Int = 2\n}\n").unwrap();
    assert!(sources::check(r).is_err());
}
fn project(settings: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir_all(t.path().join(".project/docs")).unwrap();
    fs::create_dir(t.path().join("src")).unwrap();
    fs::write(
        t.path().join(".project/docs/config.yaml"),
        format!("schema: 1\nsources:\n  roots: [src]\n  manifest: manifest.json\n{settings}"),
    )
    .unwrap();
    t
}
#[test]
fn snippets_distinct_markers_crlf_and_atomic_validation() {
    let t=project("  examples:\n    - source: src/A.kt\n      target: guide.md\n      start: '<!-- a -->'\n      end: '<!-- /a -->'\n      region_start: '// begin'\n      region_end: '// end'\n      prefix: import sample.A\n    - source: src/A.kt\n      target: guide.md\n      start: '<!-- b -->'\n      end: '<!-- /b -->'\n");
    let r = t.path();
    fs::write(r.join("src/A.kt"), "// begin\r\nclass A\r\n// end\r\n").unwrap();
    fs::write(r.join("guide.md"),"Curated\r\n<!-- a -->\r\nold\r\n<!-- /a -->\r\nBetween\r\n<!-- b -->\r\nold\r\n<!-- /b -->\r\n").unwrap();
    sources::update(r).unwrap();
    sources::check(r).unwrap();
    let first = fs::read(r.join("guide.md")).unwrap();
    sources::update(r).unwrap();
    assert_eq!(first, fs::read(r.join("guide.md")).unwrap());
    assert!(String::from_utf8(first).unwrap().contains("Curated\n"));
    fs::write(r.join("guide.md"), "missing").unwrap();
    fs::write(r.join("manifest.json"), "reviewed").unwrap();
    assert!(sources::update(r).is_err());
    assert_eq!(
        fs::read_to_string(r.join("manifest.json")).unwrap(),
        "reviewed"
    );
}
#[test]
fn added_deleted_sources_and_scope() {
    let t = project("");
    let r = t.path();
    fs::write(r.join("src/A.kt"), "class A").unwrap();
    fs::write(r.join("Other.kt"), "invalid {").unwrap();
    sources::update(r).unwrap();
    fs::write(r.join("src/B.kt"), "class B").unwrap();
    assert!(sources::check(r).is_err());
    sources::update(r).unwrap();
    fs::remove_file(r.join("src/A.kt")).unwrap();
    assert!(sources::check(r).is_err());
}
#[test]
fn traversal_collisions_syntax_and_missing_config_fail() {
    let t = project("  groups:\n    - include: ['**/*.kt']\n      output: ../outside.md\n");
    assert!(sources::update(t.path()).is_err());
    let t = project("  groups:\n    - include: ['**/*.kt']\n      output: manifest.json\n");
    assert!(sources::update(t.path()).is_err());
    let t = project("");
    fs::write(t.path().join("src/Broken.kt"), "class {").unwrap();
    let e = sources::update(t.path()).unwrap_err().to_string();
    assert!(e.contains("Broken.kt:1"), "{e}");
    assert!(sources::check(tempfile::tempdir().unwrap().path()).is_err());
}
#[test]
fn kotlin_visibility_defaults_annotations_overloads_and_grammar_compatibility() {
    let input="@Deprecated(\"old\")\nclass A private constructor(val x: Int = 2) {\n internal class Hidden { fun leaked() {} }\n protected fun secret() {}\n fun f(a: String = \"héllo\"): Int = 1\n fun f(): Int { return 2 }\n val visible: Int = run { 4 }\n companion object { const val NAME = \"sample\" }\n}\ninternal fun editor() { var open by remember { state(false) }; Call(onClick = { open = true }) { if (open) close() } }\nfun safe() { try { call() }\n catch (_: Exception) { recover() } }\n";
    let output = sources::kotlin_excerpt(input, "fixture.kt").unwrap();
    assert!(output.contains("private constructor"));
    assert!(output.contains("a: String = \"héllo\""));
    assert!(output.contains("@Deprecated"), "{output}");
    assert!(output.contains("val visible: Int"));
    assert!(!output.contains("run {"));
    assert!(!output.contains("Hidden"));
    assert!(!output.contains("secret"));
    assert!(!output.contains("return 2"));
    assert!(output.contains("fun f()"));
}
#[test]
fn contains_preserves_curated_example() {
    let t=project("  examples:\n    - source: src/A.kt\n      target: guide.md\n      start: '<!-- a -->'\n      end: '<!-- /a -->'\n      mode: contains\n");
    let r = t.path();
    fs::write(r.join("src/A.kt"), "fun main() {\n  call(1, 2)\n}").unwrap();
    let guide = "Text\n<!-- a -->\n```kotlin\ncall(1, 2)\n```\n<!-- /a -->\n";
    fs::write(r.join("guide.md"), guide).unwrap();
    sources::update(r).unwrap();
    assert_eq!(fs::read_to_string(r.join("guide.md")).unwrap(), guide);
    sources::check(r).unwrap();
    fs::write(r.join("src/A.kt"), "fun main() { call(3) }").unwrap();
    assert!(sources::update(r).is_err());
}
#[cfg(unix)]
#[test]
fn symlink_sources_rejected() {
    let t = project("");
    std::os::unix::fs::symlink("/tmp", t.path().join("src/link")).unwrap();
    assert!(sources::update(t.path()).is_err());
}
#[test]
fn initialization_preserves_config_and_remapped_selected_roots() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::create_dir_all(r.join(".project/docs")).unwrap();
    fs::create_dir_all(r.join(".project/setup")).unwrap();
    fs::create_dir_all(r.join("custom/library/src/main/kotlin")).unwrap();
    fs::create_dir_all(r.join("app/src/main/java")).unwrap();
    fs::write(
        r.join(".project/docs/config.yaml"),
        "schema: 1\nrepository: owner/repo\ntitle: Kept\ntooling_ref: v0.1.15\n",
    )
    .unwrap();
    fs::write(r.join(".project/setup/config.yaml"),"android_modules:\n  api:\n    kind: library\n    selected: true\n    directory: custom/library\n  app:\n    kind: application\n    selected: true\n    directory: app\n").unwrap();
    sources::init(r).unwrap();
    let config = fs::read_to_string(r.join(".project/docs/config.yaml")).unwrap();
    assert!(config.contains("custom/library/src/main/kotlin"));
    assert!(!config.contains("app/src/main/java"));
    assert!(config.contains("title: Kept"));
    sources::init(r).unwrap();
    assert_eq!(
        fs::read_to_string(r.join(".project/docs/config.yaml")).unwrap(),
        config
    );
}
#[test]
fn root_jvm_and_custom_initialization() {
    for source in ["src/main/java", "custom/source"] {
        let t = project("");
        let r = t.path();
        fs::write(r.join(".project/docs/config.yaml"), "schema: 1\n").unwrap();
        fs::create_dir_all(r.join(source)).unwrap();
        if source.starts_with("custom") {
            sources::init_with_roots(r, &[source.into()]).unwrap()
        } else {
            sources::init(r).unwrap()
        };
        assert!(fs::read_to_string(r.join(".project/docs/config.yaml"))
            .unwrap()
            .contains(source));
    }
}
#[test]
fn duplicate_markers_and_scenarios_rejected() {
    let t=project("  examples:\n    - source: src/A.kt\n      target: guide.md\n      start: '<!-- a -->'\n      end: '<!-- /a -->'\n    - source: src/A.kt\n      target: guide.md\n      start: '<!-- a -->'\n      end: '<!-- /a -->'\n");
    let r = t.path();
    fs::write(r.join("src/A.kt"), "class A").unwrap();
    fs::write(r.join("guide.md"), "<!-- a --><!-- /a -->").unwrap();
    assert!(sources::update(r)
        .unwrap_err()
        .to_string()
        .contains("collision"));
    fs::write(r.join("guide.md"), "<!-- a --><!-- a --><!-- /a -->").unwrap();
    assert!(sources::update(r)
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
}
#[test]
fn enum_punctuation_inferred_presets_source_context_and_nonpublic_inventory() {
    let t = project(
        "  groups:\n    - include: ['**/*.kt']\n      output: docs/api.md\n      intro: intro.md\n",
    );
    let r = t.path();
    fs::write(r.join("intro.md"), "# API\n").unwrap();
    fs::write(r.join("src/Types.kt"),"package example\nimport example.Other\nenum class Side { Front, Back;\n fun label(): String = name\n}\nclass Presets { companion object {\n val Default = Presets()\n @Deprecated(\"old\") val Old: Presets = Presets()\n} }\n").unwrap();
    fs::write(
        r.join("src/Internal.kt"),
        "package example\ninternal class Hidden\n",
    )
    .unwrap();
    sources::update(r).unwrap();
    let output = fs::read_to_string(r.join("docs/api.md")).unwrap();
    assert!(output.starts_with("# API\n\n##"), "{output}");
    assert!(output.contains("Front,\n"));
    assert!(output.contains("Back;\n"));
    assert!(output.contains("val Default = Presets()"));
    assert!(output.contains("package example\nimport example.Other"));
    assert!(output.contains("[Source](../src/Types.kt)"));
    assert!(!output.contains("Internal.kt"));
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(r.join("manifest.json")).unwrap()).unwrap();
    assert!(manifest[0]["guide"].is_null());
    assert_eq!(manifest[0]["visibility"], "nonpublic");
    sources::check(r).unwrap();
}
#[test]
fn property_accessor_visibility_is_preserved() {
    let output = sources::kotlin_excerpt(
        "class A {\n var value: Int = 0\n private set\n val computed: Int get() { return 3 }\n}\n",
        "accessors.kt",
    )
    .unwrap();
    assert!(output.contains("private set"), "{output}");
    assert!(output.contains("get()"), "{output}");
    assert!(!output.contains("return 3"));
}
#[test]
fn standalone_init_flags_and_aggregate_sources() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::create_dir_all(r.join("src/main/java")).unwrap();
    fs::write(r.join("src/main/java/A.kt"), "class A").unwrap();
    let binary = env!("CARGO_BIN_EXE_android-repo-docs");
    let status = std::process::Command::new(binary)
        .arg("--project")
        .arg(r)
        .args([
            "init",
            "--repository",
            "owner/jvm",
            "--title",
            "Independent",
            "--tooling-ref",
            "0123456789012345678901234567890123456789",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let config = fs::read_to_string(r.join(".project/docs/config.yaml")).unwrap();
    assert!(config.contains("src/main/java"));
    assert!(config.contains("0123456789012345678901234567890123456789"));
    android_repo_docs::update(r).unwrap();
    android_repo_docs::check(r).unwrap();
    assert!(r.join(".project/docs/sources.json").exists());
}
#[test]
fn invalid_output_file_type_fails_before_any_write() {
    let t = project("  groups:\n    - include: ['**/*.kt']\n      output: src\n");
    fs::write(t.path().join("manifest.json"), "reviewed").unwrap();
    assert!(sources::update(t.path()).is_err());
    assert_eq!(
        fs::read_to_string(t.path().join("manifest.json")).unwrap(),
        "reviewed"
    );
}

#[test]
fn planned_output_ancestor_collisions_preserve_reviewed_manifest() {
    for outputs in [
        ["generated", "generated/api.md"],
        ["generated/api.md", "generated"],
    ] {
        let t = project(&format!("  groups:\n    - include: ['**/*.kt']\n      output: {}\n    - include: ['**/*.kt']\n      output: {}\n", outputs[0], outputs[1]));
        let root = t.path();
        let config_path = root.join(".project/docs/config.yaml");
        let config = fs::read_to_string(&config_path)
            .unwrap()
            .replace("manifest: manifest.json", "manifest: aaa-manifest.json");
        fs::write(config_path, config).unwrap();
        fs::write(root.join("src/Public.kt"), "class Public\n").unwrap();
        fs::write(root.join("aaa-manifest.json"), "reviewed manifest\n").unwrap();
        assert!(sources::update(root).is_err());
        assert_eq!(
            fs::read_to_string(root.join("aaa-manifest.json")).unwrap(),
            "reviewed manifest\n"
        );
        assert!(!root.join("generated").exists());
    }
}

#[test]
fn annotation_values_and_comments_do_not_hide_public_declarations() {
    let input = r#"@Deprecated("use private helper")
class Public {
    @Deprecated("use internal helper")
    class Nested
    @Suppress(/* protected helper */ "warning")
    fun visible() {}
    private class Private
    internal class Internal
    protected class Protected
}
@Suppress(/* private helper */ "warning")
class AlsoPublic
private class HiddenTop
internal class InternalTop
protected class ProtectedTop
"#;
    let output = sources::kotlin_excerpt(input, "annotations.kt").unwrap();
    for name in [
        "class Public",
        "class Nested",
        "fun visible()",
        "class AlsoPublic",
    ] {
        assert!(output.contains(name), "Missing {name}: {output}");
    }
    for name in [
        "class Private",
        "class Internal",
        "class Protected",
        "class HiddenTop",
    ] {
        assert!(!output.contains(name), "Leaked {name}: {output}");
    }
}
