use std::{fs, path::Path, process::Command};
use tempfile::tempdir;
fn write(root: &Path, path: &str, xml: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, format!("<resources>{xml}</resources>")).unwrap();
}
fn check(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
        .arg("--project")
        .arg(root)
        .arg("check_localization")
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn discovers_modules_and_validates_actual_placeholders() {
    let t = tempdir().unwrap();
    let r = t.path();
    for module in ["app", "libraries/core"] {
        write(
            r,
            &format!("{module}/src/main/res/values/strings.xml"),
            r#"<string name="hello">Hello %1$s, %2$d</string><string name="brand" translatable="false">Brand</string>"#,
        );
        write(
            r,
            &format!("{module}/src/main/res/values-es/labels.xml"),
            r#"<string name="hello">%2$d, hola <b>%1$s</b></string>"#,
        );
    }
    assert!(check(r, &[]).status.success());
    write(
        r,
        "libraries/core/src/main/res/values-es/labels.xml",
        r#"<string name="hello">Hola %1$d, %2$d</string>"#,
    );
    let result = check(r, &[]);
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("hello") && error.contains("placeholders") && error.contains("libraries"),
        "{error}"
    );
}
#[test]
fn detects_missing_extra_empty_and_duplicate_keys() {
    for translated in [
        "",
        r#"<string name="a"></string>"#,
        r#"<string name="a">A</string><string name="extra">B</string>"#,
        r#"<string name="a">A</string><string name="a">B</string>"#,
    ] {
        let t = tempdir().unwrap();
        write(
            t.path(),
            "lib/src/main/res/values/a.xml",
            r#"<string name="a">A</string>"#,
        );
        write(t.path(), "lib/src/main/res/values-fr/a.xml", translated);
        assert!(!check(t.path(), &[]).status.success(), "{translated}");
    }
}
#[test]
fn supports_fallback_bcp47_nonformatted_and_ignores_build_outputs() {
    let t = tempdir().unwrap();
    let r = t.path();
    write(
        r,
        "lib/src/main/res/values/a.xml",
        r#"<string name="a" formatted="false">50% sale</string><string name="missing">Fallback</string>"#,
    );
    write(
        r,
        "lib/src/main/res/values-b+es+419/b.xml",
        r#"<string name="a" formatted="false">Oferta 50%</string>"#,
    );
    fs::create_dir_all(r.join("build/bad/src/main/res/values")).unwrap();
    fs::write(r.join("build/bad/src/main/res/values/bad.xml"), "invalid").unwrap();
    assert!(!check(r, &[]).status.success());
    assert!(check(r, &["--allow-missing"]).status.success());
}
#[test]
fn rejects_malformed_xml_and_cross_file_duplicates() {
    let t = tempdir().unwrap();
    let r = t.path();
    write(
        r,
        "app/src/main/res/values/a.xml",
        r#"<string name="a">A</string>"#,
    );
    write(
        r,
        "app/src/main/res/values/b.xml",
        r#"<string name="a">B</string>"#,
    );
    assert!(!check(r, &[]).status.success());
    fs::write(r.join("app/src/main/res/values/b.xml"), "<resources>").unwrap();
    assert!(!check(r, &[]).status.success());
}

#[test]
fn formatter_contracts_and_android_qualifiers() {
    for (source, translation, passes) in [
        ("%s %d", "%2$d %1$s", true),
        ("%s %d", "%d %s", false),
        ("%1$s %&lt;s %% %n", "%1$s %1$s %% %n", true),
        ("%1$tY", "%1$ty", false),
        ("%1$.2f", "%1$f", true),
        ("%1$s", "%1$s %2$s", false),
    ] {
        let t = tempdir().unwrap();
        let r = t.path();
        write(
            r,
            "app/src/main/res/values/a.xml",
            &format!("<string name=\"a\">{source}</string>"),
        );
        write(
            r,
            "app/src/main/res/values-es/a.xml",
            &format!("<string name=\"a\">{translation}</string>"),
        );
        // Device overlays may intentionally contain only a subset.
        write(r, "app/src/main/res/values-es-night/a.xml", "");
        write(r, "app/src/main/res/values-car/a.xml", "");
        let output = check(r, &[]);
        assert_eq!(
            output.status.success(),
            passes,
            "{source} => {translation}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn nontranslatable_strings_cannot_be_reintroduced_in_a_locale() {
    let t = tempdir().unwrap();
    let r = t.path();
    write(
        r,
        "app/src/main/res/values/a.xml",
        r#"<string name="brand" translatable="false">Brand</string>"#,
    );
    write(
        r,
        "app/src/main/res/values-fr/a.xml",
        r#"<string name="brand">Marque</string>"#,
    );
    let output = check(r, &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("non-translatable string brand"));
}
