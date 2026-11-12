//! Read-only verification of generated setup and evaluated Android inventory.
use anyhow::{ensure, Context, Result};
use std::{collections::BTreeMap, fs, path::Path};
pub(crate) fn run(root: &Path) -> Result<()> {
    crate::bootstrap::run(root, true, false)?;
    ensure!(
        !crate::workflows::update(root, crate::workflows::Mode::Check)?,
        "Workflow drift; run workflows --overwrite"
    );
    crate::publishing::generate(root, true)?;
    let actual = crate::discovery::discover(root)?;
    verify_inventory(root, &actual)?;
    println!("Setup verified");
    Ok(())
}
fn verify_inventory(root: &Path, actual: &crate::discovery::Modules) -> Result<()> {
    if let Some(saved) = crate::discovery::load(root)? {
        ensure!(
            saved.len() == actual.len(),
            "Android module inventory changed; run discover --save and review changes"
        );
        for (id, expected) in saved {
            let found = actual
                .values()
                .find(|m| m.path == expected.path)
                .with_context(|| format!("Android module {id} disappeared; run discover --save"))?;
            ensure!(
                expected.screenshots == found.screenshots
                    && expected.directory == found.directory
                    && expected.kind == found.kind
                    && expected.namespace == found.namespace
                    && expected.application_ids == found.application_ids,
                "Android module {id} changed; run discover --save and review changes"
            );
        }
    } else {
        let config: serde_yaml_ng::Value =
            serde_yaml_ng::from_slice(&fs::read(root.join(crate::discovery::CONFIG))?)?;
        let mut expected = BTreeMap::new();
        if let Some(modules) = config["modules"].as_mapping() {
            for id in modules.keys().filter_map(|id| id.as_str()) {
                expected.insert(format!(":{id}"), crate::discovery::Kind::Library);
            }
        }
        if let Some(id) = config["workflows"]["app_module"].as_str() {
            expected.insert(
                format!(":{}", id.trim_start_matches(':')),
                crate::discovery::Kind::Application,
            );
        } else if actual
            .values()
            .any(|m| m.path == ":app" && m.kind == crate::discovery::Kind::Application)
        {
            expected.insert(":app".into(), crate::discovery::Kind::Application);
        }
        ensure!(!expected.is_empty(), "No saved Android inventory or legacy module selections; run discover --save and review selections");
        ensure!(
            actual.len() == expected.len(),
            "Legacy module selection differs from evaluated Android modules; run discover --save"
        );
        for (path, kind) in expected {
            ensure!(
                actual.values().any(|m| m.path == path && m.kind == kind),
                "Legacy module {path} differs from Gradle; run discover --save"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn modules() -> crate::discovery::Modules {
        serde_yaml_ng::from_str("core: {path: ':core', directory: core, kind: library}\napp: {path: ':app', directory: app, kind: application}\n").unwrap()
    }
    fn fixture(config: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".project/setup")).unwrap();
        fs::write(root.path().join(crate::discovery::CONFIG), config).unwrap();
        root
    }
    #[test]
    fn saved_inventory_drift_is_read_only() {
        let config = format!(
            "android_modules:\n{}",
            serde_yaml_ng::to_string(&modules())
                .unwrap()
                .lines()
                .map(|s| format!("  {s}\n"))
                .collect::<String>()
        );
        let root = fixture(&config);
        assert!(verify_inventory(root.path(), &modules()).is_ok());
        let mut changed = modules();
        changed.get_mut("core").unwrap().screenshots = true;
        assert!(verify_inventory(root.path(), &changed).is_err());
        changed.get_mut("core").unwrap().directory = "renamed".into();
        assert!(verify_inventory(root.path(), &changed).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join(crate::discovery::CONFIG)).unwrap(),
            config
        );
    }
    #[test]
    fn legacy_app_id_is_mapped_to_application() {
        let root = fixture("modules: {core: Core, app: App}\nworkflows: {app_module: app}\n");
        assert!(verify_inventory(root.path(), &modules()).is_ok());
        let mut changed = modules();
        changed.get_mut("app").unwrap().kind = crate::discovery::Kind::Library;
        assert!(verify_inventory(root.path(), &changed).is_err());
    }
    #[test]
    fn legacy_default_app_and_ambiguous_config_are_actionable() {
        let root = fixture("modules: {core: Core}\n");
        assert!(verify_inventory(root.path(), &modules()).is_ok());
        let empty = fixture("modules: {}\n");
        assert!(
            verify_inventory(empty.path(), &crate::discovery::Modules::new())
                .unwrap_err()
                .to_string()
                .contains("discover --save")
        );
    }
}
