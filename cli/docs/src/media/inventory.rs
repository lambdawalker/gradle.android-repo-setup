use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
fn files(root: &Path, extension: &str, output: &mut Vec<PathBuf>) -> Result<()> {
    for item in fs::read_dir(root).with_context(|| format!("Read {}", root.display()))? {
        let item = item?;
        let kind = item.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "Symlink in check inputs: {}",
            item.path().display()
        );
        if kind.is_dir() {
            files(&item.path(), extension, output)?;
        } else if kind.is_file() && item.path().extension().is_some_and(|v| v == extension) {
            output.push(item.path());
        }
    }
    output.sort();
    Ok(())
}
pub fn image_record(directory: &Path, scenarios: &[String]) -> Result<Value> {
    ensure!(
        !scenarios.is_empty() && scenarios.iter().collect::<BTreeSet<_>>().len() == scenarios.len(),
        "Empty or duplicate screenshot scenarios"
    );
    let mut images = Vec::new();
    files(directory, "png", &mut images)?;
    let mut names = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    let mut dimensions = BTreeMap::new();
    for scenario in scenarios {
        ensure!(!scenario.is_empty(), "Empty scenario name");
        let matches: Vec<_> = images
            .iter()
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(&format!("_{scenario}_"))
            })
            .collect();
        ensure!(
            matches.len() == 1,
            "Expected exactly one screenshot for {scenario}, found {}",
            matches.len()
        );
        names.insert(
            scenario.clone(),
            matches[0]
                .strip_prefix(directory)?
                .to_string_lossy()
                .replace('\\', "/"),
        );
    }
    ensure!(
        names.values().collect::<BTreeSet<_>>().len() == images.len()
            && names.len() == images.len(),
        "Unexpected or ambiguous screenshot files"
    );
    for image in images {
        let bytes = fs::read(&image)?;
        ensure!(
            bytes.len() >= 33 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" && &bytes[12..16] == b"IHDR",
            "Invalid PNG: {}",
            image.display()
        );
        let width = u32::from_be_bytes(bytes[16..20].try_into()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into()?);
        ensure!(width > 0 && height > 0, "Invalid PNG dimensions");
        let name = image
            .strip_prefix(directory)?
            .to_string_lossy()
            .replace('\\', "/");
        hashes.insert(name.clone(), format!("{:x}", Sha256::digest(&bytes)));
        dimensions.insert(name, json!({"width":width,"height":height}));
    }
    Ok(json!({"scenarios":names,"images":hashes,"dimensions":dimensions}))
}

/// Compatibility inventory contract shared with the setup executable.
pub fn generate(root: &Path, module: &str) -> Result<()> {
    let config: Value =
        serde_json::from_slice(&fs::read(root.join(".project/verification/checks.json"))?)?;
    let spec = &config["screenshots"][module];
    let directory = super::path(root, super::text(spec, "directory")?)?;
    let scenarios: Vec<String> = serde_json::from_value(spec["scenarios"].clone())?;
    let mut record = image_record(&directory, &scenarios)?;
    if let Some(metadata) = spec["metadata"].as_object() {
        for (key, value) in metadata {
            ensure!(
                ![
                    "schema",
                    "source",
                    "module",
                    "scenarios",
                    "images",
                    "dimensions"
                ]
                .contains(&key.as_str()),
                "Reserved screenshot metadata key: {key}"
            );
            record[key] = value.clone();
        }
    }
    let source = String::from_utf8(super::git(root, &["rev-parse", "HEAD"])?)?;
    record["schema"] = json!(1);
    record["source"] = json!(source.trim());
    record["module"] = json!(module);
    let output = directory.join("render-manifest.json");
    let mut tmp = tempfile::NamedTempFile::new_in(&directory)?;
    use std::io::Write;
    writeln!(tmp, "{}", serde_json::to_string_pretty(&record)?)?;
    tmp.persist(output)?;
    Ok(())
}
