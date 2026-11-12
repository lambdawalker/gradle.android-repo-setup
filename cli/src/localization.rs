use anyhow::{bail, ensure, Context, Result};
use regex::Regex;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

struct StringResource {
    text: String,
    translatable: bool,
    formatted: bool,
}

// Never traverse symlinks or generated/dependency directories.
fn resource_roots(path: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path).with_context(|| format!("Read {}", path.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || matches!(name.as_ref(), "build" | "target" | "node_modules" | "bin")
        {
            continue;
        }
        let child = entry.path();
        if name == "res"
            && child
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|n| n == "src")
        {
            found.push(child);
        } else {
            resource_roots(&child, found)?;
        }
    }
    Ok(())
}

fn read_strings(directory: &Path) -> Result<BTreeMap<String, StringResource>> {
    let mut strings = BTreeMap::new();
    if !directory.exists() {
        return Ok(strings);
    }
    let mut files = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    files.sort_by_key(|entry| entry.file_name());
    for entry in files {
        if !entry.file_type()?.is_file() || entry.path().extension().is_none_or(|ext| ext != "xml")
        {
            continue;
        }
        let path = entry.path();
        let xml = fs::read_to_string(&path)?;
        let document = roxmltree::Document::parse(&xml)
            .with_context(|| format!("Invalid XML: {}", path.display()))?;
        ensure!(
            document.root_element().has_tag_name("resources"),
            "Expected <resources>: {}",
            path.display()
        );
        for node in document
            .root_element()
            .children()
            .filter(|n| n.has_tag_name("string"))
        {
            let name = node
                .attribute("name")
                .context("String resource has no name")?;
            let resource = StringResource {
                text: node
                    .descendants()
                    .filter(|n| n.is_text())
                    .filter_map(|n| n.text())
                    .collect(),
                translatable: node.attribute("translatable") != Some("false"),
                formatted: node.attribute("formatted") != Some("false"),
            };
            ensure!(
                strings.insert(name.to_owned(), resource).is_none(),
                "Duplicate string key {name}: {}",
                path.display()
            );
        }
    }
    Ok(strings)
}

// Java Formatter arguments: explicit/implicit indices and '<' reuse; %% and %n consume none.
fn placeholders(text: &str, pattern: &Regex) -> Result<Vec<(usize, String)>> {
    let mut result = Vec::new();
    let mut implicit = 0;
    let mut previous = None;
    for capture in pattern.captures_iter(text) {
        let conversion = &capture[4];
        if conversion == "%" || conversion == "n" {
            continue;
        }
        let index = if capture[2].contains('<') {
            previous.context("Placeholder '<' has no preceding argument")?
        } else if let Some(index) = capture.get(1) {
            index.as_str().parse::<usize>()?
        } else {
            implicit += 1;
            implicit
        };
        ensure!(index > 0, "Placeholder index must be positive");
        previous = Some(index);
        result.push((
            index,
            format!(
                "{}{}",
                capture.get(3).map_or("", |m| m.as_str()),
                conversion
            ),
        ));
    }
    result.sort();
    Ok(result)
}

pub(crate) fn check(root: &Path, allow_missing: bool) -> Result<()> {
    let locale = Regex::new(
        r"^values-(?:[a-z]{2,3}(?:-r(?:[A-Z]{2}|[0-9]{3}))?|b\+[A-Za-z]{2,3}(?:\+[A-Za-z0-9]{2,8})*)$",
    )?;
    let format = Regex::new(r"%(?:(\d+)\$)?([-#+ 0,(<]*)(?:\d+)?(?:\.\d+)?([tT])?([a-zA-Z%])")?;
    let mut roots = Vec::new();
    resource_roots(root, &mut roots)?;
    roots.sort();
    let mut errors = Vec::new();
    let mut locales = 0;
    for res in &roots {
        let english = read_strings(&res.join("values"))?;
        for entry in fs::read_dir(res)? {
            let entry = entry?;
            if entry.file_name() == "values-car"
                || !entry.file_type()?.is_dir()
                || !locale.is_match(&entry.file_name().to_string_lossy())
            {
                continue;
            }
            locales += 1;
            let path = entry.path();
            let translated = read_strings(&path)?;
            let label = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            for (name, original) in &english {
                if !original.translatable {
                    continue;
                }
                match translated.get(name) {
                    None if !allow_missing => {
                        errors.push(format!("{label}: missing string {name}"))
                    }
                    Some(value) => {
                        if value.text.trim().is_empty() {
                            errors.push(format!("{label}: empty translation {name}"));
                        }
                        if original.formatted
                            && (!value.formatted
                                || placeholders(&original.text, &format)?
                                    != placeholders(&value.text, &format)?)
                        {
                            errors.push(format!("{label}: placeholders differ for {name}"));
                        }
                    }
                    _ => {}
                }
            }
            for name in translated.keys() {
                match english.get(name) {
                    None => errors.push(format!("{label}: extra string {name}")),
                    Some(value) if !value.translatable => errors.push(format!(
                        "{label}: non-translatable string {name} was translated"
                    )),
                    _ => {}
                }
            }
        }
    }
    errors.sort();
    if !errors.is_empty() {
        bail!("Localization validation failed:\n{}", errors.join("\n"));
    }
    println!(
        "Validated named strings in {} resource roots and {locales} locale directories",
        roots.len()
    );
    Ok(())
}
