use crate::release::{coordinates, git, verify, SUFFIXES};
use anyhow::{bail, ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Pending(String);

impl std::fmt::Display for Pending {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Pending {}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let result = git(root, args)?;
    ensure!(
        result.status.success(),
        "Git could not read the release reservation or source ({})",
        args[0]
    );
    Ok(String::from_utf8(result.stdout)?.trim_end().to_owned())
}

fn read_reservation(
    root: &Path,
    module: &str,
    version: &str,
    destination: &str,
    scope: &str,
    source: Option<&str>,
) -> Result<Value> {
    let current = coordinates(root, module, version, destination, true)?;
    let reference = format!("refs/tags/release-pending/{scope}{module}/{version}");
    let local = git_text(root, &["rev-parse", &reference])?;
    ensure!(
        git_text(root, &["cat-file", "-t", &local])? == "tag",
        "Journal must be annotated"
    );
    let remote = git_text(root, &["ls-remote", "--refs", "origin", &reference])?;
    ensure!(
        remote == format!("{local}\t{reference}"),
        "Remote reservation differs"
    );
    // Read the pinned object rather than the ref, which may move after the comparison.
    let annotated = git_text(root, &["cat-file", "-p", &local])?;
    let (_, message) = annotated.split_once("\n\n").context("Missing journal")?;
    let record: Value = serde_json::from_str(message)?;
    let expected_source = match source {
        Some(source) => source.to_owned(),
        None => git_text(root, &["rev-parse", "HEAD"])?,
    };
    let journal_source = record["source"]
        .as_str()
        .context("Missing journal source")?;
    ensure!(
        Regex::new(r"\A[0-9a-f]{40}\z")?.is_match(journal_source)
            && git_text(root, &["rev-parse", &format!("{local}^{{commit}}")])? == journal_source
            && record["version"] == version,
        "Journal source/version mismatch"
    );
    ensure!(
        journal_source == expected_source,
        "Confirmation source differs from reservation"
    );

    let fields = record.as_object().context("Expected journal object")?;
    ensure!(
        fields
            .get("repository")
            .is_some_and(|value| value == destination)
            || (!fields.contains_key("repository") && destination == "maven-central"),
        "Journal repository differs from selection"
    );
    ensure!(
        fields
            .get("repository_url")
            .is_some_and(|value| value == current.repository_url)
            || (!fields.contains_key("repository_url") && destination == "maven-central"),
        "Journal destination differs from configured URL"
    );
    ensure!(
        record["module"] == module
            && record["group"] == current.group
            && record["artifact"] == current.artifact
            && fields
                .get("companions")
                .is_none_or(|value| value.is_null() || value == &json!([])),
        "Journal must contain exactly the selected module and coordinates"
    );
    if let Some(source_tag) = fields.get("source_tag") {
        ensure!(
            source_tag == &format!("{module}/v{version}-{destination}")
                && fields
                    .get("legacy_tag")
                    .is_none_or(|value| value.is_null() || value == false || value == ""),
            "Invalid destination source tag"
        );
    }
    if destination == "jitpack" {
        let expected = if fields.contains_key("source_tag") {
            current
                .consumer_version
                .context("Missing JitPack coordinates")?
        } else {
            format!("{module}~v{version}")
        };
        ensure!(
            record["consumer_version"] == expected,
            "JitPack record coordinates mismatch"
        );
    } else {
        ensure!(
            fields
                .get("consumer_version")
                .is_none_or(|value| value == version),
            "Maven Central record coordinates mismatch"
        );
    }
    ensure!(
        matches!(
            record["phase"].as_str(),
            Some("reserved-before-upload" | "confirmed-public")
        ),
        "Invalid phase"
    );
    let hashes = record["sha256"]
        .as_object()
        .context("Missing publication hashes")?;
    let digest = Regex::new(r"\A[0-9a-f]{64}\z")?;
    ensure!(
        SUFFIXES.iter().all(|suffix| hashes.contains_key(*suffix)
            || (destination == "jitpack" && *suffix == ".module"))
            && hashes
                .iter()
                .all(|(suffix, value)| SUFFIXES.contains(&suffix.as_str())
                    && value.as_str().is_some_and(|value| digest.is_match(value))),
        "Incomplete publication hashes"
    );
    Ok(record)
}

fn verify_public(
    record: &Value,
    destination: &str,
    repository_url: &str,
    fetch: &impl Fn(&str, bool) -> Result<Option<Vec<u8>>>,
) -> Result<Value> {
    let required = |url: &str| -> Result<Vec<u8>> {
        fetch(url, false)?.ok_or_else(|| Pending("Public artifact is missing".into()).into())
    };
    let group = record["group"].as_str().context("Missing group")?;
    let artifact = record["artifact"].as_str().context("Missing artifact")?;
    let version = record["consumer_version"]
        .as_str()
        .or_else(|| record["version"].as_str())
        .context("Missing version")?;
    let base = format!(
        "{repository_url}/{group_path}/{artifact}/{version}/{artifact}-{version}",
        group_path = group.replace('.', "/")
    );
    let jitpack = destination == "jitpack";
    if jitpack {
        // Requesting the POM triggers a build. The status API alone does not.
        required(&format!("{base}.pom"))?;
        let api = format!("{repository_url}/api/builds/{group}/{artifact}/{version}");
        let provenance: Value = serde_json::from_slice(&required(&api)?)?;
        match provenance["status"]
            .as_str()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("none" | "queued" | "pending" | "building" | "running") => {
                return Err(Pending("JitPack build is not ready".into()).into());
            }
            Some("ok") => {}
            _ => bail!("JitPack build failed or returned an unknown status"),
        }
        ensure!(
            provenance["commit"] == record["source"],
            "JitPack build is not successful at the reserved source commit"
        );
        for (field, expected) in [
            ("group", group),
            ("artifact", artifact),
            ("version", version),
        ] {
            ensure!(
                provenance.get(field).is_none_or(|value| value == expected),
                "JitPack build API identity mismatch"
            );
        }
    }
    let mut hashes = serde_json::Map::new();
    for suffix in SUFFIXES {
        let url = format!("{base}{suffix}");
        let data = if jitpack && suffix == ".module" {
            let Some(data) = fetch(&url, true)? else {
                continue;
            };
            data
        } else {
            required(&url)?
        };
        verify(&data, suffix, group, artifact, version, false)
            .with_context(|| format!("Invalid public artifact {suffix}"))?;
        let hash = format!("{:x}", Sha256::digest(&data));
        if !jitpack {
            ensure!(
                record["sha256"][suffix] == hash,
                "Public artifact hash mismatch: {suffix}"
            );
            let signature = required(&format!("{url}.asc"))?;
            let marker = b"BEGIN PGP SIGNATURE";
            ensure!(
                signature
                    .windows(marker.len())
                    .any(|window| window == marker),
                "Missing signature: {suffix}"
            );
        }
        hashes.insert(suffix.into(), Value::String(hash));
    }
    Ok(Value::Object(hashes))
}

fn registry_error(error: ureq::Error) -> anyhow::Error {
    if matches!(
        error,
        ureq::Error::Timeout(_)
            | ureq::Error::Io(_)
            | ureq::Error::ConnectionFailed
            | ureq::Error::HostNotFound
    ) {
        Pending(format!("Public registry request failed: {error}")).into()
    } else {
        error.into()
    }
}

pub(crate) fn confirm(
    root: &Path,
    module: &str,
    version: &str,
    destination: &str,
    source: Option<&str>,
    timeout: Duration,
) -> Result<()> {
    let (scope, repository_url) = match destination {
        "maven-central" => ("", crate::release::CENTRAL),
        "jitpack" => ("jitpack/", crate::release::JITPACK),
        _ => bail!("Unknown release repository"),
    };
    let mut record = read_reservation(root, module, version, destination, scope, source)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .https_only(true)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let fetch = |url: &str, optional: bool| -> Result<Option<Vec<u8>>> {
        let mut response = agent.get(url).call().map_err(registry_error)?;
        match response.status().as_u16() {
            200 => Ok(Some(
                response
                    .body_mut()
                    .with_config()
                    .limit(64 * 1024 * 1024)
                    .read_to_vec()
                    .map_err(registry_error)?,
            )),
            404 if optional => Ok(None),
            code @ (404 | 429 | 500..=599) => {
                Err(Pending(format!("Public registry returned HTTP {code}")).into())
            }
            code => bail!("Public registry returned HTTP {code}"),
        }
    };
    let started = Instant::now();
    loop {
        match verify_public(&record, destination, repository_url, &fetch) {
            Ok(hashes) => {
                record["sha256"] = hashes;
                break;
            }
            Err(error) if error.is::<Pending>() => {
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(error.context(
                        "Public artifacts not confirmed; preserve journal and retry finalization",
                    ));
                }
                std::thread::sleep(remaining.min(Duration::from_secs(20)));
            }
            Err(error) => return Err(error),
        }
    }
    record["phase"] = json!("confirmed-public");
    let path = root.join(format!("build/{scope}confirmed-{module}.json"));
    let directory = path.parent().context("Missing output directory")?;
    fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(&mut file, &record)?;
    writeln!(file)?;
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn artifacts(group: &str, artifact: &str, version: &str) -> [Vec<u8>; 5] {
        let pom = format!("<project><groupId>{group}</groupId><artifactId>{artifact}</artifactId><version>{version}</version><packaging>aar</packaging><name>Demo</name><description>Fixture</description><url>https://example.invalid</url><licenses><license><name>Test</name><url>https://example.invalid</url></license></licenses><developers><developer><id>test</id><name>Test</name></developer></developers><scm><url>https://example.invalid</url><connection>scm:git:https://example.invalid</connection></scm><dependencies><dependency><artifactId>foundation</artifactId></dependency><dependency><artifactId>ui</artifactId></dependency></dependencies></project>");
        let classes = archive(&[(
            "com/apexfission/android/democommon/Demo.class",
            &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61],
        )]);
        [
            pom.into_bytes(),
            archive(&[
                ("AndroidManifest.xml", b"<manifest/>"),
                ("classes.jar", &classes),
            ]),
            archive(&[("Demo.kt", b"class Demo")]),
            archive(&[("index.html", b"<title>Demo</title>")]),
            serde_json::to_vec(
                &json!({"component": {"group": group, "module": artifact, "version": version}}),
            )
            .unwrap(),
        ]
    }

    #[test]
    fn central_checks_all_hashes_and_signature_markers() {
        let artifacts = artifacts("com.example.android", "demo", "1.2.3");
        let hashes: serde_json::Map<_, _> = SUFFIXES
            .iter()
            .zip(&artifacts)
            .map(|(suffix, data)| {
                (
                    suffix.to_string(),
                    json!(format!("{:x}", Sha256::digest(data))),
                )
            })
            .collect();
        let mut record = json!({"group": "com.example.android", "artifact": "demo", "version": "1.2.3", "sha256": hashes});
        let fetch = |url: &str, _: bool| {
            assert!(url
                .starts_with("https://example.invalid/com/example/android/demo/1.2.3/demo-1.2.3"));
            Ok(Some(if url.ends_with(".asc") {
                b"-----BEGIN PGP SIGNATURE-----".to_vec()
            } else {
                let index = SUFFIXES
                    .iter()
                    .position(|suffix| url.ends_with(suffix))
                    .unwrap();
                artifacts[index].clone()
            }))
        };
        let verify = |record: &Value, fetch: &_| {
            verify_public(record, "maven-central", "https://example.invalid", fetch)
        };
        assert_eq!(verify(&record, &fetch).unwrap(), record["sha256"]);
        let bad_signature = verify_public(
            &record,
            "maven-central",
            "https://example.invalid",
            &|url, optional| {
                if url.ends_with(".asc") {
                    Ok(Some(b"bad signature".to_vec()))
                } else {
                    fetch(url, optional)
                }
            },
        )
        .unwrap_err();
        assert!(bad_signature.to_string().contains("Missing signature"));
        record["sha256"][".aar"] = json!("0".repeat(64));
        assert!(verify(&record, &fetch)
            .unwrap_err()
            .to_string()
            .contains("hash mismatch"));
    }

    #[test]
    fn jitpack_distinguishes_pending_failed_and_wrong_source_and_allows_missing_module() {
        let artifacts = artifacts("com.github.example", "android-demo", "demo~v1.2.3-jitpack");
        let record = json!({"group": "com.github.example", "artifact": "android-demo", "consumer_version": "demo~v1.2.3-jitpack", "source": "a".repeat(40)});
        let verify = |status: &str, commit: &str| {
            let requests = std::cell::Cell::new(0);
            verify_public(
                &record,
                "jitpack",
                "https://example.invalid",
                &|url, optional| {
                    let index = requests.get();
                    requests.set(index + 1);
                    if index == 0 {
                        assert!(url.ends_with(".pom"));
                    }
                    if index == 1 {
                        assert_eq!(url, "https://example.invalid/api/builds/com.github.example/android-demo/demo~v1.2.3-jitpack");
                        return Ok(Some(
                            serde_json::to_vec(&json!({"status": status, "commit": commit}))
                                .unwrap(),
                        ));
                    }
                    if optional {
                        assert!(url.ends_with(".module"));
                        return Ok(None);
                    }
                    let index = SUFFIXES
                        .iter()
                        .position(|suffix| url.ends_with(suffix))
                        .unwrap();
                    Ok(Some(artifacts[index].clone()))
                },
            )
        };
        assert!(verify("building", &"a".repeat(40))
            .unwrap_err()
            .is::<Pending>());
        assert!(!verify("error", &"a".repeat(40))
            .unwrap_err()
            .is::<Pending>());
        assert!(verify("ok", &"b".repeat(40))
            .unwrap_err()
            .to_string()
            .contains("reserved source"));
        assert_eq!(
            verify("OK", &"a".repeat(40))
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            4
        );
    }
}
