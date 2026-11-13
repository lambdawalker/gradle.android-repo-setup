//! Discovery uses evaluated Gradle plugin identities, never directory-name guesses.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Component, Path},
    process::{Command, Stdio},
};

pub(crate) const CONFIG: &str = ".project/setup/config.yaml";
pub(crate) type Modules = BTreeMap<String, Module>;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Library,
    Application,
    #[serde(rename = "jvm-library")]
    JvmLibrary,
    Other,
}
impl Kind {
    pub(crate) fn is_library(&self) -> bool {
        matches!(self, Self::Library | Self::JvmLibrary)
    }
    pub(crate) fn is_jvm(&self) -> bool {
        matches!(self, Self::JvmLibrary)
    }
}
fn selected() -> bool {
    true
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Module {
    pub path: String,
    pub directory: String,
    pub kind: Kind,
    #[serde(default = "selected")]
    pub selected: bool,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub application_ids: Vec<String>,
    #[serde(default)]
    pub screenshots: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    schema: u32,
    modules: Vec<Module>,
}

pub(crate) fn validate(modules: &Modules) -> Result<()> {
    let id_pattern = regex::Regex::new(r"\A[A-Za-z0-9_-][A-Za-z0-9_.-]*\z")?;
    let path_pattern = regex::Regex::new(
        r"\A:(?:[A-Za-z0-9_-][A-Za-z0-9_.-]*(?::[A-Za-z0-9_-][A-Za-z0-9_.-]*)*)?\z",
    )?;
    let mut paths = BTreeSet::new();
    for (id, m) in modules {
        ensure!(
            m.kind != Kind::Other,
            "android_modules must contain only Android libraries, apps, and JVM libraries"
        );
        ensure!(
            id_pattern.is_match(id) && id != "." && id != "..",
            "Invalid module ID: {id}"
        );
        ensure!(
            path_pattern.is_match(&m.path),
            "Invalid Gradle path: {}",
            m.path
        );
        ensure!(paths.insert(&m.path), "Duplicate Gradle path: {}", m.path);
        ensure!(
            !Path::new(&m.directory).is_absolute()
                && Path::new(&m.directory)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
                && !m.directory.contains(['\n', '\r', '\\']),
            "Module directory must be inside the project: {}",
            m.directory
        );
        if !m.application_ids.is_empty() {
            crate::associations::validate_ids(&m.application_ids)?;
        }
    }
    Ok(())
}
pub(crate) fn load(root: &Path) -> Result<Option<Modules>> {
    let path = root.join(CONFIG);
    if !path.exists() {
        return Ok(None);
    }
    let config: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&fs::read(path)?)?;
    if config["android_modules"].is_null() {
        return Ok(None);
    }
    let modules = serde_yaml_ng::from_value(config["android_modules"].clone())?;
    validate(&modules)?;
    Ok(Some(modules))
}
pub(crate) fn module(root: &Path, id: &str) -> Result<Module> {
    if let Some(modules) = load(root)? {
        return modules
            .get(id)
            .cloned()
            .with_context(|| format!("Unknown module: {id}"));
    }
    let modules = BTreeMap::from([(
        id.to_owned(),
        Module {
            path: format!(":{id}"),
            directory: id.into(),
            kind: Kind::Library,
            selected: true,
            namespace: None,
            application_ids: vec![],
            screenshots: false,
        },
    )]);
    validate(&modules)?;
    Ok(modules[id].clone())
}
fn parse_report(bytes: &[u8], previous: Option<&Modules>) -> Result<Modules> {
    let report: Report =
        serde_json::from_slice(bytes).context("Invalid Gradle discovery report")?;
    ensure!(report.schema == 1, "Unsupported discovery schema");
    let mut modules = Modules::new();
    for mut m in report.modules {
        if m.kind == Kind::Other {
            continue;
        }
        if m.directory.is_empty() {
            m.directory = ".".into();
        }
        let old = previous.and_then(|all| all.iter().find(|(_, old)| old.path == m.path));
        let id = old.map(|(id, _)| id.clone()).unwrap_or_else(|| {
            if m.path == ":" {
                "root".into()
            } else {
                m.path.trim_start_matches(':').replace(':', "--")
            }
        });
        if let Some((_, old)) = old {
            m.selected = old.selected;
        }
        ensure!(
            modules.insert(id.clone(), m).is_none(),
            "Module ID collision: {id}; configure distinct IDs in android_modules"
        );
    }
    validate(&modules)?;
    Ok(modules)
}
pub(crate) fn discover(root: &Path) -> Result<Modules> {
    let previous = load(root)?;
    let temporary = tempfile::tempdir()?;
    let init = temporary.path().join("discover.gradle");
    let report = temporary.path().join("modules.json");
    fs::write(&init, include_str!("../gradle/discover.gradle"))?;
    let wrapper = root.join(if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    });
    ensure!(
        wrapper.is_file(),
        "No Gradle wrapper found. Run from the Gradle build root with --project."
    );
    let output = Command::new(wrapper)
        .current_dir(root)
        .args([
            "--no-daemon",
            "--no-configuration-cache",
            "--console=plain",
            "-I",
        ])
        .arg(&init)
        .arg(format!("-DandroidRepoSetup.report={}", report.display()))
        .arg(":androidRepoSetupDiscover")
        .stdin(Stdio::null())
        .output()
        .context("Could not run Gradle module discovery")?;
    ensure!(output.status.success(), "Gradle discovery failed; selections were not changed. Fix project configuration and retry.\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    parse_report(
        &fs::read(report).context("Gradle did not produce a discovery report")?,
        previous.as_ref(),
    )
}
fn initial_config(root: &Path) -> Result<serde_yaml_ng::Value> {
    let remote = Command::new("git")
        .current_dir(root)
        .args(["remote", "get-url", "origin"])
        .output()?;
    ensure!(remote.status.success(), "Set a GitHub origin remote or create .project/setup/config.yaml with repository and tooling_ref before setup");
    let remote = String::from_utf8(remote.stdout)?;
    let repository = remote
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| remote.trim().strip_prefix("git@github.com:"))
        .or_else(|| remote.trim().strip_prefix("ssh://git@github.com/"))
        .context("Expected a GitHub origin remote")?
        .trim_end_matches(".git");
    ensure!(
        regex::Regex::new(r"\A[A-Za-z0-9_-]+/[A-Za-z0-9_.-]+\z")?.is_match(repository),
        "Invalid GitHub repository identity"
    );
    let title = repository.split('/').next_back().unwrap();
    Ok(serde_yaml_ng::to_value(
        serde_json::json!({"repository":repository,"title":title,"modules":{},"tooling_ref":format!("v{}",env!("CARGO_PKG_VERSION")),"workflows":{"title":title}}),
    )?)
}
pub(crate) fn save(root: &Path, modules: &Modules) -> Result<()> {
    validate(modules)?;
    let path = root.join(CONFIG);
    for parent in [
        root.join(".project"),
        root.join(".project/setup"),
        path.clone(),
    ] {
        if let Ok(metadata) = fs::symlink_metadata(&parent) {
            ensure!(
                !metadata.file_type().is_symlink(),
                "Refusing symlinked setup path"
            );
        }
    }
    let mut config: serde_yaml_ng::Value = if path.exists() {
        serde_yaml_ng::from_slice(&fs::read(&path)?)?
    } else {
        initial_config(root)?
    };
    fs::create_dir_all(path.parent().unwrap())?;
    config["android_modules"] = serde_yaml_ng::to_value(modules)?;
    if config["modules"].is_null() {
        config["modules"] = serde_yaml_ng::Value::Mapping(Default::default());
    }
    for (id, m) in modules.iter().filter(|(_, m)| m.kind.is_library()) {
        // Retain historical labels and deselected library histories.
        if config["modules"][id].is_null() {
            config["modules"][id] = id.clone().into();
        }
        if config["verification"].is_null() && config["module_verification"][id].is_null() {
            if let Some(namespace) = &m.namespace {
                if config["module_verification"].is_null() {
                    config["module_verification"] =
                        serde_yaml_ng::Value::Mapping(Default::default());
                }
                config["module_verification"][id] = serde_yaml_ng::to_value(
                    serde_json::json!({"class_prefix": format!("{}/", namespace.replace('.', "/")), "required_api_artifacts": []}),
                )?;
            }
        }
    }
    let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    staged.write_all(serde_yaml_ng::to_string(&config)?.as_bytes())?;
    staged.persist(&path)?;
    let inventory_path = root.join(".project/setup/modules.properties");
    if let Ok(metadata) = fs::symlink_metadata(&inventory_path) {
        ensure!(
            !metadata.file_type().is_symlink(),
            "Refusing symlinked module inventory"
        );
    }
    let mut inventory_file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    inventory_file.write_all(inventory(root, modules)?.as_bytes())?;
    inventory_file.persist(inventory_path)?;
    let registry = root.join(".project/setup/repositories.yml");
    if !registry.exists() && modules.values().any(|m| m.kind.is_library() && m.selected) {
        let legacy = root.join("publishing/repositories.yml");
        let contents = if legacy.is_file() {
            fs::read_to_string(legacy)?
        } else {
            "repositories:\n  maven-central: {environment: maven-central, publisher: central}\n  jitpack: {environment: jitpack, publisher: jitpack}\n".into()
        };
        fs::write(registry, contents)?;
    }
    Ok(())
}
pub(crate) fn inventory(root: &Path, modules: &Modules) -> Result<String> {
    let mut text = String::from(
        "# Generated from .project/setup/config.yaml; run android-repo-setup workflows.\n",
    );
    let config: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&fs::read(root.join(CONFIG))?)?;
    let repo = config["repository"]
        .as_str()
        .context("Missing repository")?;
    let name = repo.split('/').next_back().unwrap();
    ensure!(
        regex::Regex::new(r"\A[A-Za-z0-9_.-]+\z")?.is_match(name),
        "Invalid repository name"
    );
    text.push_str(&format!("repository.name={name}\n"));
    for (id, m) in modules {
        text.push_str(&format!(
            "{id}.path={}\n{id}.directory={}\n{id}.selected={}\n{id}.kind={}\n",
            m.path,
            m.directory.replace('\\', "\\\\").replace(' ', "\\ "),
            m.selected,
            match m.kind {
                Kind::Library => "library",
                Kind::Application => "application",
                Kind::JvmLibrary => "jvm-library",
                Kind::Other => "other",
            }
        ));
    }
    Ok(text)
}
pub(crate) fn select(root: &Path) -> Result<Option<Modules>> {
    setup_message!("Discovering Android and JVM modules through Gradle…");
    let mut modules = discover(root)?;
    ensure!(
        !modules.is_empty(),
        "No Android library, application, or JVM library plugins found in this Gradle build."
    );
    let entries = modules
        .iter()
        .map(|(id, m)| {
            (
                format!(
                    "{} — {}",
                    m.path,
                    if m.kind.is_jvm() {
                        "JVM library"
                    } else if m.kind.is_library() {
                        "Android library"
                    } else {
                        "App"
                    }
                ),
                format!("{} · {id}", m.directory),
                m.selected,
            )
        })
        .collect();
    let Some(indices) = crate::ui::select_modules(entries)? else {
        return Ok(None);
    };
    for (i, m) in modules.values_mut().enumerate() {
        m.selected = indices.contains(&i);
    }
    save(root, &modules)?;
    Ok(Some(modules))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_root_jvm_library_and_preserves_selection() {
        let report =
            br#"{"schema":1,"modules":[{"path":":","directory":"","kind":"jvm-library"}]}"#;
        let mut modules = parse_report(report, None).unwrap();
        assert_eq!(modules["root"].directory, ".");
        assert!(modules["root"].kind.is_library());
        assert!(modules["root"].kind.is_jvm());
        modules.get_mut("root").unwrap().selected = false;
        assert!(!parse_report(report, Some(&modules)).unwrap()["root"].selected);
        assert_eq!(
            serde_json::to_string(&modules["root"].kind).unwrap(),
            "\"jvm-library\""
        );
    }
    #[test]
    fn rejects_collision_traversal_and_duplicates() {
        for modules in [
            r#"[{"path":":a:b","directory":"a","kind":"library"},{"path":":a--b","directory":"b","kind":"application"}]"#,
            r#"[{"path":":a","directory":"../escape","kind":"library"}]"#,
            r#"[{"path":":a","directory":"a","kind":"library"},{"path":":a","directory":"b","kind":"library"}]"#,
        ] {
            assert!(parse_report(
                format!("{{\"schema\":1,\"modules\":{modules}}}").as_bytes(),
                None
            )
            .is_err());
        }
    }
}
