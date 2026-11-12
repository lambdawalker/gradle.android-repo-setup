use crate::vault::{parent, private_dir, regular_file_exists, SetupConfig};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub fn path(config: &SetupConfig) -> PathBuf {
    let mut filename = config.state_path().into_os_string();
    filename.push(".associations.json");
    PathBuf::from(filename)
}

pub fn validate_ids(ids: &[String]) -> Result<Vec<String>> {
    ensure!(
        !ids.is_empty() && ids.len() <= 100,
        "Supply 1–100 Android application IDs."
    );
    let pattern = regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+$")?;
    let mut normalized = BTreeSet::new();
    for id in ids {
        let id = id.trim();
        ensure!(
            pattern.is_match(id),
            "Invalid Android application ID: {id:?}"
        );
        normalized.insert(id.to_owned());
    }
    Ok(normalized.into_iter().collect())
}

/// Explicit project configuration takes precedence over remembered wizard answers.
/// An empty result means the wizard must ask; no package is inferred from Gradle text.
pub fn application_ids(root: &Path, saved: &[String]) -> Result<Vec<String>> {
    #[derive(Default, Deserialize)]
    struct Signing {
        application_ids: Option<Vec<String>>,
    }
    #[derive(Deserialize)]
    struct Config {
        #[serde(default)]
        signing: Signing,
    }
    let configured = match fs::read(root.join(".project/setup/config.yaml")) {
        Ok(bytes) => {
            serde_yaml_ng::from_slice::<Config>(&bytes)
                .context("Could not read signing.application_ids from .project/setup/config.yaml.")?
                .signing
                .application_ids
        }
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let discovered = crate::discovery::load(root)?
        .map(|modules| {
            modules
                .into_values()
                .filter(|m| m.selected && m.kind == crate::discovery::Kind::Application)
                .flat_map(|m| m.application_ids)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match configured {
        None if !discovered.is_empty() => validate_ids(&discovered),
        Some(ids) => validate_ids(&ids),
        None if saved.is_empty() => Ok(Vec::new()),
        None => validate_ids(saved),
    }
}

/// The caller supplies the selected signing certificate in DER form, never the keystore.
pub fn write(path: &Path, ids: &[String], certificate: &[u8]) -> Result<()> {
    let ids = validate_ids(ids)?;
    ensure!(
        !certificate.is_empty() && certificate.len() <= 1024 * 1024,
        "Invalid signing certificate size."
    );
    let fingerprint = Sha256::digest(certificate)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    let applications = ids
        .iter()
        .map(|id| {
            json!({
                "packageName": id,
                "sha256CertFingerprints": [&fingerprint],
            })
        })
        .collect::<Vec<_>>();
    let mut data = serde_json::to_vec_pretty(&applications)?;
    data.push(b'\n');
    regular_file_exists(path)?;
    private_dir(parent(path))?;
    let mut temporary = NamedTempFile::new_in(parent(path))?;
    temporary.write_all(&data)?;
    temporary.as_file().sync_all()?;
    regular_file_exists(path)?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .context("Could not replace the public Android association file.")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::fs;

    const ABC_SHA256: &str = "BA:78:16:BF:8F:01:CF:EA:41:41:40:DE:5D:AE:22:23:B0:03:61:A3:96:17:7A:9C:B4:10:FF:61:F2:00:15:AD";

    #[test]
    fn public_export_uses_the_import_schema_and_certificate_digest() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let output = scratch.path().join("setup.associations.json");
        write(&output, &["com.example.app".into()], b"abc")?;
        let value: Value = serde_json::from_slice(&fs::read(output)?)?;
        assert_eq!(
            value,
            json!([{"packageName":"com.example.app", "sha256CertFingerprints":[ABC_SHA256]}])
        );
        Ok(())
    }

    #[test]
    fn ids_are_normalized_and_validated_without_guessing() -> Result<()> {
        assert_eq!(
            validate_ids(&[
                "com.example.z".into(),
                " com.example.a ".into(),
                "com.example.z".into()
            ])?,
            vec!["com.example.a", "com.example.z"]
        );
        for ids in [
            vec![],
            vec!["".into()],
            vec!["app".into()],
            vec!["com..app".into()],
            vec!["com.example-app".into()],
            vec!["com.1app".into()],
            vec!["com.example/app".into()],
            vec!["com.example.app".into(); 101],
        ] {
            assert!(validate_ids(&ids).is_err(), "accepted {ids:?}");
        }
        Ok(())
    }

    #[test]
    fn configured_ids_take_precedence_over_saved_answers() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let saved = vec!["com.example.old".into()];
        assert!(application_ids(scratch.path(), &[])?.is_empty());
        assert_eq!(application_ids(scratch.path(), &saved)?, saved);
        fs::create_dir_all(scratch.path().join(".project/setup"))?;
        let config = scratch.path().join(".project/setup/config.yaml");
        fs::write(&config, "repository: owner/project\n")?;
        assert_eq!(application_ids(scratch.path(), &saved)?, saved);
        fs::write(
            &config,
            "signing:\n  application_ids: [com.example.release, com.example.debug]\n",
        )?;
        assert_eq!(
            application_ids(scratch.path(), &saved)?,
            vec!["com.example.debug", "com.example.release"]
        );
        for content in [
            "signing: {application_ids: []}",
            "signing: {application_ids: [bad-id]}",
            "signing: {application_ids: com.example.app}",
            "signing: [",
        ] {
            fs::write(&config, content)?;
            assert!(
                application_ids(scratch.path(), &saved).is_err(),
                "accepted {content}"
            );
        }
        Ok(())
    }

    #[test]
    fn refreshed_export_replaces_old_identities_and_preserves_on_error() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let output = scratch.path().join("setup.associations.json");
        write(&output, &["com.example.old".into()], b"old")?;
        write(&output, &["com.example.new".into()], b"abc")?;
        let original = fs::read(&output)?;
        let value: Value = serde_json::from_slice(&original)?;
        assert_eq!(
            value,
            json!([{"packageName":"com.example.new", "sha256CertFingerprints":[ABC_SHA256]}])
        );
        assert!(write(&output, &["bad-id".into()], b"abc").is_err());
        assert!(write(&output, &["com.example.new".into()], b"").is_err());
        assert_eq!(fs::read(&output)?, original);
        Ok(())
    }

    #[test]
    fn exports_follow_vault_location_and_isolate_signing_environments() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let config = |env: &str, override_path| {
            SetupConfig::new(
                "owner/Project".into(),
                env.into(),
                "main".into(),
                Some(scratch.path().to_owned()),
                override_path,
                None,
            )
        };
        let release = config("release", None)?;
        let debug = config("debug", None)?;
        write(&path(&release), &["com.example.app".into()], b"abc")?;
        let original = fs::read(path(&release))?;
        write(&path(&debug), &["com.example.debug".into()], b"debug")?;
        assert_eq!(fs::read(path(&release))?, original);
        assert_ne!(path(&release), path(&debug));
        assert_eq!(
            path(&release).parent().unwrap(),
            scratch.path().join("project/secrets")
        );
        let custom = config("release", Some(scratch.path().join("custom/signing.vault")))?;
        write(&path(&custom), &["com.example.app".into()], b"abc")?;
        assert!(scratch
            .path()
            .join("custom/signing.vault.associations.json")
            .is_file());
        Ok(())
    }

    #[test]
    fn export_paths_do_not_collide_for_custom_vaults_with_the_same_stem() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let config = |name: &str| {
            SetupConfig::new(
                "owner/project".into(),
                "release".into(),
                "main".into(),
                Some(scratch.path().to_owned()),
                Some(scratch.path().join(name)),
                None,
            )
        };
        let first = config("release.vault")?;
        let second = config("release.enc")?;
        write(&path(&first), &["com.example.first".into()], b"first")?;
        let original = fs::read(path(&first))?;
        write(&path(&second), &["com.example.second".into()], b"second")?;
        assert_eq!(
            fs::read(path(&first))?,
            original,
            "another vault replaced this export"
        );
        Ok(())
    }

    #[test]
    fn export_rejects_nonregular_destinations() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let output = scratch.path().join("directory");
        fs::create_dir(&output)?;
        assert!(write(&output, &["com.example.app".into()], b"abc").is_err());
        #[cfg(unix)]
        {
            let target = scratch.path().join("private-file");
            fs::write(&target, "untouched")?;
            let link = scratch.path().join("symlink.json");
            std::os::unix::fs::symlink(&target, &link)?;
            assert!(write(&link, &["com.example.app".into()], b"abc").is_err());
            assert_eq!(fs::read_to_string(&target)?, "untouched");
        }
        Ok(())
    }
}
