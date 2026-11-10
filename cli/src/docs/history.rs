use super::{
    children, examples, json_path, matches, read_json, string, string_or, validate_record,
    version_key, Config,
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(crate) const HISTORY: &str = "docs/releases/history";

fn confirmed_record<'a>(config: &Config, record: &'a Value) -> Result<&'a str> {
    ensure!(
        record["phase"] == "confirmed-public",
        "Only confirmed releases belong in the archive"
    );
    let module = string(record, "module")?;
    ensure!(
        config.modules().any(|(name, _)| name == module),
        "Unknown archive module"
    );
    let target = string_or(record, "repository", "maven-central")?;
    validate_record(target, module, record)?;
    Ok(target)
}

fn archive_name(record: &Value) -> Result<String> {
    let version = string(record, "version")?;
    if record.get("source_tag").is_some() {
        Ok(format!("{version}-{}", string(record, "repository")?))
    } else {
        Ok(version.to_owned())
    }
}

fn archive_path(record: &Value) -> Result<PathBuf> {
    Ok(Path::new(HISTORY)
        .join(string(record, "module")?)
        .join(format!("{}.json", archive_name(record)?)))
}

fn destinations(entry: &Value) -> Result<&Map<String, Value>> {
    entry["destinations"]
        .as_object()
        .filter(|map| !map.is_empty())
        .context("Invalid archive schema or empty destinations")
}

fn validate(config: &Config, entry: &Value) -> Result<()> {
    ensure!(
        entry["schema"] == 1,
        "Invalid archive schema or empty destinations"
    );
    let records = destinations(entry)?;
    ensure!(
        matches("[0-9a-f]{40}", string(entry, "documentation_ref")?),
        "Documentation revision must be an immutable commit SHA"
    );
    if entry.get("translation_tree").is_some() {
        ensure!(
            matches("[0-9a-f]{40}", string(entry, "translation_tree")?),
            "Translation tree must be an immutable SHA"
        );
    }
    let mut scope = None;
    for (target, record) in records {
        ensure!(
            confirmed_record(config, record)? == target
                && ["module", "version", "source"]
                    .iter()
                    .all(|key| record[key] == entry[key]),
            "Archive identity conflict"
        );
        let path = archive_path(record)?;
        ensure!(
            scope.as_ref().is_none_or(|first| first == &path),
            "Archive destination scope conflict"
        );
        scope = Some(path);
    }
    Ok(())
}

pub(crate) fn validate_archive(root: &Path, entry: &Value) -> Result<()> {
    validate(&Config::load(root)?, entry)
}

pub(super) fn route_version(entry: &Value) -> Result<String> {
    archive_name(destinations(entry)?.values().next().unwrap())
}

pub(crate) fn archive_record(root: &Path, record: &Value) -> Result<PathBuf> {
    archive(root, &Config::load(root)?, record)
}

fn archive(root: &Path, config: &Config, record: &Value) -> Result<PathBuf> {
    let target = confirmed_record(config, record)?;
    let path = archive_path(record)?;
    let absolute = root.join(&path);
    let mut entry = if absolute.exists() {
        let entry = read_json(absolute.clone())?;
        validate(config, &entry)?;
        entry
    } else {
        json!({
            "schema": 1,
            "module": record["module"],
            "version": record["version"],
            "source": record["source"],
            "documentation_ref": record["source"],
            "destinations": {},
        })
    };
    ensure!(
        entry["source"] == record["source"],
        "Archive source conflict"
    );
    if let Some(existing) = entry["destinations"].get(target) {
        ensure!(existing == record, "Archive confirmation conflict");
    }
    entry["destinations"][target] = record.clone();
    validate(config, &entry)?;
    fs::create_dir_all(absolute.parent().unwrap())?;
    fs::write(absolute, serde_json::to_string_pretty(&entry)? + "\n")?;
    Ok(path)
}

fn latest_records(root: &Path) -> Result<Vec<Value>> {
    let folder = root.join("docs/releases");
    let mut paths = Vec::new();
    for child in children(&folder)? {
        if json_path(&child) {
            paths.push(child.clone());
        }
        if child.is_dir() && child.file_name().is_some_and(|name| name != "history") {
            paths.extend(children(&child)?.into_iter().filter(|path| json_path(path)));
        }
    }
    paths.sort();
    let mut records = Vec::new();
    for path in paths {
        let record = read_json(path)?;
        if !record.is_null() {
            records.push(record);
        }
    }
    Ok(records)
}

pub(crate) fn seed(root: &Path) -> Result<()> {
    seed_archive(root, &Config::load(root)?)
}

fn seed_archive(root: &Path, config: &Config) -> Result<()> {
    for record in latest_records(root)? {
        archive(root, config, &record)?;
    }
    Ok(())
}

pub(super) fn catalog(root: &Path, config: &Config) -> Result<Vec<Value>> {
    let mut entries = Vec::new();
    let history = root.join(HISTORY);
    for module in children(&history)?.into_iter().filter(|path| path.is_dir()) {
        for path in children(&module)?
            .into_iter()
            .filter(|path| json_path(path))
        {
            let entry = read_json(path.clone())?;
            validate(config, &entry)?;
            let expected = history
                .join(string(&entry, "module")?)
                .join(format!("{}.json", route_version(&entry)?));
            ensure!(path == expected, "Archive filename identity conflict");
            entries.push(entry);
        }
    }
    entries.sort_by(|left, right| {
        let key = |entry: &Value| {
            version_key(entry["version"].as_str().unwrap())
                .unwrap()
                .map(|(length, part)| (length, part.to_owned()))
        };
        left["module"]
            .as_str()
            .cmp(&right["module"].as_str())
            .then_with(|| key(left).cmp(&key(right)))
    });
    Ok(entries)
}

pub(super) fn verify(root: &Path, config: &Config, catalog: &[Value]) -> Result<()> {
    let mut entries = BTreeMap::new();
    for entry in catalog {
        entries.insert((string(entry, "module")?, route_version(entry)?), entry);
    }
    for record in latest_records(root)? {
        let target = confirmed_record(config, &record)?;
        let entry = entries.get(&(string(&record, "module")?, archive_name(&record)?));
        ensure!(
            entry.and_then(|entry| entry["destinations"].get(target)) == Some(&record),
            "Latest confirmation missing from archive; run documentation_history.py seed"
        );
    }
    Ok(())
}

const SPANISH_REPLACEMENTS: &[(&str, &str)] = &[
    ("#### Version catalog", "#### Catálogo de versiones"),
    ("In `settings.gradle.kts`:", "En `settings.gradle.kts`:"),
    (
        "In the app's `build.gradle.kts`:",
        "En el archivo `build.gradle.kts` de la aplicación:",
    ),
    ("In `settings.gradle`:", "En `settings.gradle`:"),
    (
        "In the app's `build.gradle`:",
        "En el archivo `build.gradle` de la aplicación:",
    ),
    (
        "Use the dependency repositories shown above. Add to `gradle/libs.versions.toml`:",
        "Usa los repositorios indicados arriba. Añade lo siguiente a `gradle/libs.versions.toml`:",
    ),
    (
        "Then use this instead of the direct dependency in the app's `build.gradle.kts`:",
        "Después, usa este alias en lugar de la dependencia directa en `build.gradle.kts`:",
    ),
    (
        "Add these repositories and dependency to `pom.xml`:",
        "Añade estos repositorios y esta dependencia a `pom.xml`:",
    ),
];

pub(super) fn installation(entry: &Value, lang: &str) -> Result<String> {
    let spanish = lang == "es";
    let module = string(entry, "module")?;
    let version = string(entry, "version")?;
    let (title, intro) = if spanish {
        ("Instalación", "Elige **un** destino y **una** sintaxis de dependencia. Todos los destinos que aparecen aquí confirmaron esta versión del mismo código.")
    } else {
        ("Installation", "Choose **one** destination and **one** dependency syntax. Every destination below confirmed this version of the same source.")
    };
    let mut text = format!("# {title}: {module} {version}\n\n{intro}\n");
    for (target, record) in destinations(entry)? {
        let mut examples = examples(module, record)?;
        if spanish {
            for (original, replacement) in SPANISH_REPLACEMENTS {
                ensure!(
                    examples.contains(original),
                    "Missing Spanish translation source phrase: {original:?}"
                );
                examples = examples.replace(original, replacement);
            }
        }
        text.push_str(&format!("\n## {target}\n\n{examples}\n"));
    }
    Ok(text)
}

pub(crate) fn history(root: &Path, command: &str) -> Result<()> {
    let config = Config::load(root)?;
    if command == "seed" {
        seed_archive(root, &config)?;
    }
    let mut entries = catalog(root, &config)?;
    verify(root, &config, &entries)?;
    match command {
        "seed" | "verify" => Ok(()),
        "export" => {
            for entry in &mut entries {
                entry["route_version"] = json!(route_version(entry)?);
                entry["installation"] = json!({
                    "en": installation(entry, "en")?,
                    "es": installation(entry, "es")?
                });
            }
            println!("{}", serde_json::to_string(&entries)?);
            Ok(())
        }
        _ => bail!("Unknown documentation history operation"),
    }
}
