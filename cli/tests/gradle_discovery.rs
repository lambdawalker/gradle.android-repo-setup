//! Run with GRADLE_BIN=/absolute/path/to/gradle cargo test --test gradle_discovery -- --ignored.
//! Requires Java 17+ and Gradle 9.6; no Android SDK or external dependencies are used.
use std::{fs, path::Path, process::Command};

fn write(root: &Path, path: &str, value: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

#[test]
#[ignore = "requires an installed Gradle; set GRADLE_BIN and run explicitly"]
fn discovers_evaluated_plugins_nested_paths_remapped_directories_and_app_ids() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    write(
        root,
        "settings.gradle",
        r#"
rootProject.name = 'discovery-fixture'
include ':libs:core', ':other:core', ':app', ':plain', ':jvmapp'
project(':libs:core').projectDir = file('custom/implementation')
"#,
    );
    write(root, "build.gradle", "plugins { id 'java-library' }");
    write(root, "libs/build.gradle", "");
    write(
        root,
        "buildSrc/build.gradle",
        r#"
plugins { id 'java-gradle-plugin' }
gradlePlugin {
  plugins {
    library { id = 'com.android.library'; implementationClass = 'fixture.LibraryPlugin' }
    application { id = 'com.android.application'; implementationClass = 'fixture.ApplicationPlugin' }
    convention { id = 'fixture.library-convention'; implementationClass = 'fixture.ConventionPlugin' }
    screenshot { id = 'com.android.compose.screenshot'; implementationClass = 'fixture.ScreenshotPlugin' }
  }
}
"#,
    );
    write(
        root,
        "buildSrc/src/main/java/fixture/LibraryPlugin.java",
        r#"
package fixture;
import org.gradle.api.*;
public class LibraryPlugin implements Plugin<Project> {
  public void apply(Project p) { p.getExtensions().add("android", new Android()); }
  public static class Android { public String getNamespace() { return "fixture.library"; } }
}
"#,
    );
    write(
        root,
        "buildSrc/src/main/java/fixture/ConventionPlugin.java",
        r#"
package fixture;
import org.gradle.api.*;
public class ConventionPlugin implements Plugin<Project> {
  public void apply(Project p) { p.getPluginManager().apply("com.android.library"); }
}
"#,
    );
    write(
        root,
        "buildSrc/src/main/java/fixture/ScreenshotPlugin.java",
        r#"
package fixture;
import org.gradle.api.*;
public class ScreenshotPlugin implements Plugin<Project> { public void apply(Project p) {} }
"#,
    );
    write(
        root,
        "buildSrc/src/main/java/fixture/ApplicationPlugin.java",
        r#"
package fixture;
import org.gradle.api.*;
public class ApplicationPlugin implements Plugin<Project> {
  public void apply(Project p) {
    p.getExtensions().add("android", new Android(p));
    p.getExtensions().add("androidComponents", new Components(p));
  }
  public static class Components {
    private final Project project;
    public Components(Project project) { this.project = project; }
    public Components selector() { return this; }
    public Components all() { return this; }
    public void onVariants(Components selector, Action<Variant> callback) {
      callback.execute(new Variant(project, "fixture.app"));
      callback.execute(new Variant(project, "fixture.app.debug"));
    }
  }
  public static class Android {
    private final DomainObjectSet<Variant> variants;
    public Android(Project p) {
      variants = p.getObjects().domainObjectSet(Variant.class);
      variants.add(new Variant(p, "fixture.app"));
      variants.add(new Variant(p, "fixture.app.debug"));
    }
    public String getNamespace() { return "fixture.app.namespace"; }
    public DomainObjectSet<Variant> getApplicationVariants() { return variants; }
  }
  public static class Variant {
    private final org.gradle.api.provider.Provider<String> id;
    public Variant(Project p, String id) { this.id = p.provider(() -> id); }
    public org.gradle.api.provider.Provider<String> getApplicationId() { return id; }
  }
}
"#,
    );
    write(
        root,
        "custom/implementation/build.gradle",
        "plugins { id 'fixture.library-convention'; id 'com.android.compose.screenshot' }",
    );
    write(
        root,
        "other/core/build.gradle",
        "plugins { id 'com.android.library' }",
    );
    write(
        root,
        "app/build.gradle",
        "plugins { id 'com.android.application'; id 'java-library' }",
    );
    write(root, "plain/build.gradle", "");
    write(
        root,
        "jvmapp/build.gradle",
        "plugins { id 'application'; id 'java-library' }",
    );
    let report = root.join("report.json");
    let output = Command::new(std::env::var_os("GRADLE_BIN").expect("set GRADLE_BIN"))
        .current_dir(root)
        .args([
            "--offline",
            "--no-daemon",
            "--max-workers=2",
            "--console=plain",
            "-I",
        ])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("gradle/discover.gradle"))
        .arg(format!("-DandroidRepoSetup.report={}", report.display()))
        .arg("androidRepoSetupDiscover")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(value["schema"], 1);
    let modules = value["modules"].as_array().unwrap();
    let module = |path: &str| modules.iter().find(|m| m["path"] == path).unwrap();
    assert_eq!(module(":libs:core")["directory"], "custom/implementation");
    assert_eq!(module(":libs:core")["kind"], "library");
    assert_eq!(module(":libs:core")["namespace"], "fixture.library");
    assert_eq!(module(":libs:core")["screenshots"], true);
    assert_eq!(module(":other:core")["kind"], "library");
    assert_eq!(module(":app")["kind"], "application");
    assert_eq!(
        module(":app")["application_ids"],
        serde_json::json!(["fixture.app", "fixture.app.debug"])
    );
    assert_eq!(module(":plain")["kind"], "other");
    assert_eq!(module(":jvmapp")["kind"], "other");
    assert_eq!(module(":")["kind"], "jvm-library");
    assert!(module(":")["namespace"].is_null());
}
