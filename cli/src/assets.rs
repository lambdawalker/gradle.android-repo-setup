//! Immutable Git/LFS asset contracts shared by preparation and artifact verification.
use crate::release::{git, VerificationConfig};
use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Source {
    #[default]
    Git,
}
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Validator {
    #[default]
    None,
    Tflite,
}
fn yes() -> bool {
    true
}
fn patterns() -> Vec<String> {
    vec!["**/*".into()]
}
fn archive_dir() -> String {
    "assets".into()
}
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssetSet {
    pub directory: String,
    #[serde(default = "patterns")]
    pub include: Vec<String>,
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub hydrate_lfs: bool,
    #[serde(default = "yes")]
    pub required: bool,
    #[serde(default)]
    pub allow_empty_files: bool,
    #[serde(default)]
    pub validator: Validator,
    #[serde(default = "archive_dir")]
    pub archive_directory: String,
}
#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
pub(crate) struct AssetRules {
    pub validator: Validator,
    pub allow_empty_files: bool,
}
#[derive(Debug, Serialize)]
pub(crate) struct Asset {
    pub source_path: String,
    pub sha256: String,
    pub size: u64,
    #[serde(skip)]
    pointer: Option<Vec<u8>>,
    #[serde(skip)]
    hydrate: bool,
    #[serde(skip)]
    validator: Validator,
    #[serde(skip)]
    allow_empty: bool,
}
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|s| {
            !matches!(s, "" | "." | ".." | ".git")
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_- .".contains(c))
        })
}
fn matcher(pattern: &str) -> Result<Regex> {
    ensure!(
        !pattern.is_empty()
            && !pattern.starts_with('/')
            && pattern.split('/').all(|s| !matches!(s, "" | "." | ".."))
            && pattern
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-. /*?".contains(c)),
        "Invalid asset glob: {pattern}"
    );
    let mut result = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    result.push_str("(?:.*/)?");
                } else {
                    result.push_str(".*");
                }
            }
            '*' => result.push_str("[^/]*"),
            '?' => result.push_str("[^/]"),
            c => result.push_str(&regex::escape(&c.to_string())),
        }
    }
    result.push('$');
    Ok(Regex::new(&result)?)
}
impl AssetSet {
    fn matches(&self, path: &str) -> Result<bool> {
        for pattern in &self.include {
            if matcher(pattern)?.is_match(path) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            safe_path(&self.directory),
            "Invalid asset directory: {}",
            self.directory
        );
        ensure!(
            safe_path(&self.archive_directory)
                && (self.archive_directory == "assets"
                    || self.archive_directory.starts_with("assets/")),
            "Asset archive_directory must be within assets/"
        );
        ensure!(!self.include.is_empty(), "Asset include must not be empty");
        for p in &self.include {
            matcher(p)?;
        }
        Ok(())
    }
}
fn sets(contract: &VerificationConfig) -> Result<Vec<AssetSet>> {
    ensure!(
        contract.required_assets.is_none() || contract.assets.is_empty(),
        "Use assets or required_assets, not both"
    );
    if let Some(old) = &contract.required_assets {
        ensure!(
            old.suffix.starts_with('.')
                && old.suffix.len() > 1
                && old
                    .suffix
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
            "Invalid required asset suffix"
        );
        return Ok(vec![AssetSet {
            directory: old.directory.clone(),
            include: vec![format!("**/*{}", old.suffix)],
            source: Source::Git,
            hydrate_lfs: false,
            required: true,
            allow_empty_files: false,
            validator: Validator::None,
            archive_directory: archive_dir(),
        }]);
    }
    Ok(contract.assets.clone())
}
pub(crate) fn validate(contract: &VerificationConfig) -> Result<()> {
    for set in sets(contract)? {
        set.validate()?;
    }
    Ok(())
}
fn pointer(data: &[u8]) -> Result<Option<(String, u64)>> {
    if !data.starts_with(b"version https://git-lfs.github.com/spec/") {
        return Ok(None);
    }
    let text = std::str::from_utf8(data).context("Invalid LFS pointer encoding")?;
    let re = Regex::new(
        r"\Aversion https://git-lfs.github.com/spec/v1\noid sha256:([0-9a-f]{64})\nsize (0|[1-9][0-9]*)\n?\z",
    )?;
    let m = re
        .captures(text)
        .context("Malformed or unsupported LFS pointer")?;
    Ok(Some((m[1].into(), m[2].parse()?)))
}
fn format_check(bytes: &[u8], validator: Validator, allow_empty: bool) -> Result<()> {
    ensure!(
        allow_empty || !bytes.is_empty(),
        "Empty asset is not allowed"
    );
    ensure!(pointer(bytes)?.is_none(), "Unresolved Git LFS pointer");
    if matches!(validator, Validator::Tflite) {
        ensure!(
            bytes.len() >= 8 && &bytes[4..8] == b"TFL3",
            "Asset is not identified as TensorFlow Lite (TFL3)"
        );
    }
    Ok(())
}
/// Build expectations from Git objects only. No network or worktree reads.
pub(crate) fn manifest(
    root: &Path,
    source: &str,
    contract: &VerificationConfig,
) -> Result<BTreeMap<String, Asset>> {
    validate(contract)?;
    let mut result = BTreeMap::new();
    for set in sets(contract)? {
        let output = git(
            root,
            &[
                "ls-tree",
                "-r",
                "-z",
                source,
                "--",
                &format!(":(literal){}", set.directory),
            ],
        )?;
        ensure!(
            output.status.success(),
            "Could not enumerate immutable assets"
        );
        let mut count = 0;
        for line in output.stdout.split(|b| *b == 0).filter(|l| !l.is_empty()) {
            let line = std::str::from_utf8(line)?;
            let (metadata, path) = line.split_once('\t').context("Invalid Git tree entry")?;
            let relative = path
                .strip_prefix(&format!("{}/", set.directory))
                .context("Asset outside directory")?;
            if !set.matches(relative)? {
                continue;
            }
            ensure!(safe_path(path), "Unsupported asset path: {path}");
            ensure!(
                metadata.starts_with("100644 blob ") || metadata.starts_with("100755 blob "),
                "Asset must be a regular Git file: {path}"
            );
            let object = metadata
                .split_whitespace()
                .nth(2)
                .context("Missing asset object")?;
            let blob = git(root, &["cat-file", "blob", object])?;
            ensure!(blob.status.success(), "Could not read asset: {path}");
            let lfs = pointer(&blob.stdout)?;
            let (hash, size) = if let Some((hash, size)) = &lfs {
                ensure!(
                    set.allow_empty_files || *size > 0,
                    "Empty LFS asset is not allowed"
                );
                (hash.clone(), *size)
            } else {
                format_check(&blob.stdout, set.validator, set.allow_empty_files)?;
                (
                    format!("{:x}", Sha256::digest(&blob.stdout)),
                    blob.stdout.len() as u64,
                )
            };
            let archive = format!("{}/{relative}", set.archive_directory);
            ensure!(
                !contract
                    .forbidden_asset_suffixes
                    .iter()
                    .any(|s| archive.ends_with(s)),
                "Required asset is forbidden: {archive}"
            );
            let asset = Asset {
                source_path: path.into(),
                sha256: hash,
                size,
                pointer: lfs.map(|_| blob.stdout),
                hydrate: set.hydrate_lfs,
                validator: set.validator,
                allow_empty: set.allow_empty_files,
            };
            ensure!(
                result.insert(archive.clone(), asset).is_none(),
                "Overlapping asset archive path: {archive}"
            );
            count += 1;
        }
        ensure!(
            !set.required || count > 0,
            "Required asset set has no matching committed files: {}",
            set.directory
        );
    }
    Ok(result)
}
pub(crate) fn resolve(root: &Path, source: &str, contract: &mut VerificationConfig) -> Result<()> {
    let assets = manifest(root, source, contract)?;
    contract.asset_sha256 = assets
        .iter()
        .map(|(p, a)| (p.clone(), a.sha256.clone()))
        .collect();
    contract.asset_rules = assets
        .iter()
        .map(|(p, a)| {
            (
                p.clone(),
                AssetRules {
                    validator: a.validator,
                    allow_empty_files: a.allow_empty,
                },
            )
        })
        .collect();
    contract.asset_sizes = assets.into_iter().map(|(p, a)| (p, a.size)).collect();
    Ok(())
}
fn regular_ancestors(root: &Path, path: &str) -> Result<()> {
    let mut current = root.to_path_buf();
    for part in path.split('/') {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(m) => ensure!(
                !m.file_type().is_symlink(),
                "Asset path contains symlink: {path}"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn verify_bytes(asset: &Asset, bytes: &[u8]) -> Result<()> {
    format_check(bytes, asset.validator, asset.allow_empty)?;
    ensure!(
        bytes.len() as u64 == asset.size && format!("{:x}", Sha256::digest(bytes)) == asset.sha256,
        "Asset differs from immutable source: {}",
        asset.source_path
    );
    Ok(())
}
fn check_extra(
    root: &Path,
    dir: &Path,
    set: &AssetSet,
    assets: &BTreeMap<String, Asset>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for item in fs::read_dir(dir)? {
        let item = item?;
        let path = item.path();
        let ty = item.file_type()?;
        ensure!(
            !ty.is_symlink(),
            "Asset directory contains symlink: {}",
            path.display()
        );
        if ty.is_dir() {
            check_extra(root, &path, set, assets)?;
        } else {
            let relative = path
                .strip_prefix(root.join(&set.directory))?
                .to_str()
                .context("Non-UTF8 asset path")?
                .replace('\\', "/");
            if set.matches(&relative)? {
                let name = format!("{}/{relative}", set.archive_directory);
                ensure!(
                    assets.get(&name).is_some_and(
                        |asset| asset.source_path == format!("{}/{relative}", set.directory)
                    ),
                    "Untracked or unexpected asset: {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}
pub(crate) fn prepare(
    root: &Path,
    module: &str,
    source: Option<&str>,
) -> Result<BTreeMap<String, Asset>> {
    let selected = crate::discovery::module(root, module)?;
    ensure!(
        selected.selected && selected.kind.is_library(),
        "Module is not a selected library"
    );
    let head = git(root, &["rev-parse", "HEAD"])?;
    ensure!(head.status.success(), "Could not resolve HEAD");
    let head = std::str::from_utf8(&head.stdout)?.trim();
    ensure!(
        source.is_none_or(|s| s == head),
        "Asset source must match checked-out HEAD"
    );
    let contract = crate::release::verification_config(root, head, module)?;
    let assets = manifest(root, head, &contract)?;
    for set in sets(&contract)? {
        regular_ancestors(root, &set.directory)?;
        check_extra(root, &root.join(&set.directory), &set, &assets)?;
    }
    let mut hydrate = Vec::new();
    for asset in assets.values() {
        regular_ancestors(root, &asset.source_path)?;
        let bytes = fs::read(root.join(&asset.source_path));
        match bytes {
            Ok(bytes) if asset.pointer.as_ref() == Some(&bytes) => {
                ensure!(
                    asset.hydrate,
                    "LFS asset requires hydrate_lfs: true: {}",
                    asset.source_path
                );
                hydrate.push(asset.source_path.clone());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && asset.pointer.is_some() => {
                ensure!(
                    asset.hydrate,
                    "Missing LFS asset requires hydrate_lfs: true: {}",
                    asset.source_path
                );
                hydrate.push(asset.source_path.clone());
            }
            Ok(bytes) => verify_bytes(asset, &bytes)?,
            Err(e) => return Err(e.into()),
        }
    }
    if !hydrate.is_empty() {
        // Paths are validated literal names; no wildcard/metacharacter can broaden LFS scope.
        let output = git(
            root,
            &[
                "-c",
                "filter.lfs.clean=git-lfs clean -- %f",
                "-c",
                "filter.lfs.smudge=git-lfs smudge -- %f",
                "lfs",
                "pull",
                &format!("--include={}", hydrate.join(",")),
                "--exclude=",
            ],
        )?;
        ensure!(
            output.status.success(),
            "Git LFS hydration failed; install git-lfs and verify access to the repository remote"
        );
    }
    for asset in assets.values() {
        regular_ancestors(root, &asset.source_path)?;
        verify_bytes(asset, &fs::read(root.join(&asset.source_path))?)?;
    }
    Ok(assets)
}
pub(crate) fn verify_archive_entry(
    contract: &VerificationConfig,
    name: &str,
    bytes: &[u8],
) -> Result<()> {
    let mut matches = false;
    for set in sets(contract)? {
        if let Some(relative) = name.strip_prefix(&format!("{}/", set.archive_directory)) {
            if set.matches(relative)? {
                matches = true;
            }
        }
    }
    if !matches && !contract.asset_sha256.contains_key(name) {
        return Ok(());
    }
    if let Some(rules) = contract.asset_rules.get(name) {
        format_check(bytes, rules.validator, rules.allow_empty_files)?;
    }
    let hash = contract
        .asset_sha256
        .get(name)
        .with_context(|| format!("Unexpected AAR asset absent from immutable source: {name}"))?;
    ensure!(
        format!("{:x}", Sha256::digest(bytes)) == *hash,
        "AAR asset hash mismatch: {name}"
    );
    if let Some(size) = contract.asset_sizes.get(name) {
        ensure!(
            bytes.len() as u64 == *size,
            "AAR asset size mismatch: {name}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packaged_assets_enforce_manifest_format_and_size() -> Result<()> {
        let mut contract: VerificationConfig = serde_yaml_ng::from_str(
            "assets:\n  - directory: models\n    include: ['**/*.tflite']\n    validator: tflite\n",
        )?;
        let bytes = b"\0\0\0\0TFL3model";
        let name = "assets/model.tflite";
        contract.asset_rules.insert(
            name.into(),
            AssetRules {
                validator: Validator::Tflite,
                allow_empty_files: false,
            },
        );
        contract
            .asset_sha256
            .insert(name.into(), format!("{:x}", Sha256::digest(bytes)));
        contract.asset_sizes.insert(name.into(), bytes.len() as u64);
        verify_archive_entry(&contract, name, bytes)?;
        assert!(verify_archive_entry(&contract, "assets/extra.tflite", bytes).is_err());
        assert!(verify_archive_entry(&contract, name, b"\0\0\0\0TFL3wrong").is_err());
        contract.asset_sizes.insert(name.into(), 999);
        assert!(verify_archive_entry(&contract, name, bytes).is_err());
        let bad = b"not tflite";
        contract
            .asset_sha256
            .insert(name.into(), format!("{:x}", Sha256::digest(bad)));
        contract.asset_sizes.insert(name.into(), bad.len() as u64);
        assert!(verify_archive_entry(&contract, name, bad).is_err());
        Ok(())
    }
    #[test]
    fn archive_checks_use_the_originating_sets_rules() -> Result<()> {
        let mut contract: VerificationConfig = serde_yaml_ng::from_str("assets:\n  - directory: models\n    validator: tflite\n  - directory: dictionaries\n    archive_directory: assets/dictionaries\n    allow_empty_files: true\n")?;
        let name = "assets/dictionaries/words.txt";
        contract
            .asset_sha256
            .insert(name.into(), format!("{:x}", Sha256::digest(b"")));
        contract.asset_sizes.insert(name.into(), 0);
        contract.asset_rules.insert(
            name.into(),
            AssetRules {
                validator: Validator::None,
                allow_empty_files: true,
            },
        );
        verify_archive_entry(&contract, name, b"")?;
        Ok(())
    }
    #[test]
    fn glob_and_pointer_validation() -> Result<()> {
        let pattern = matcher("**/*.tflite")?;
        assert!(pattern.is_match("model.tflite"));
        assert!(pattern.is_match("nested/model.tflite"));
        assert!(!pattern.is_match("model.tflite.bak"));
        assert!(!matcher("*.txt")?.is_match("nested/a.txt"));
        assert!(matcher("../*").is_err());
        assert!(pointer(b"version https://git-lfs.github.com/spec/v1\ninvalid").is_err());
        assert!(format_check(b"", Validator::None, false).is_err());
        Ok(())
    }
}
