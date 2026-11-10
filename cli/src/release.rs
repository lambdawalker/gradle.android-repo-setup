use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Cursor, Read};
use std::path::Path;
use std::process::Command;
use zip::ZipArchive;

pub(super) const SUFFIXES: [&str; 5] = [".pom", ".aar", "-sources.jar", "-javadoc.jar", ".module"];
pub(super) const CENTRAL: &str = "https://repo.maven.apache.org/maven2";
pub(super) const JITPACK: &str = "https://jitpack.io";

#[derive(Deserialize)]
struct ProjectConfig {
    repository: String,
    modules: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub(crate) struct Record<'a> {
    module: &'a str,
    version: &'a str,
    source: &'a str,
    group: String,
    artifact: String,
    sha256: BTreeMap<&'static str, String>,
    phase: &'static str,
    workflow_run: &'a str,
    repository: &'a str,
    repository_url: &'static str,
    source_tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    consumer_version: Option<String>,
}

pub(crate) fn check_local<'a>(
    root: &Path,
    module: &'a str,
    version: &'a str,
    source: &'a str,
    destination: &'a str,
    workflow_run: &'a str,
) -> Result<Record<'a>> {
    ensure!(
        Regex::new(r"\A[0-9a-f]{40}\z")?.is_match(source),
        "Invalid source SHA"
    );
    let head = git(root, &["rev-parse", "HEAD"])?;
    ensure!(
        head.status.success() && String::from_utf8(head.stdout)?.trim() == source,
        "Checkout differs from source"
    );
    let diff = git(
        root,
        &["diff", "--quiet", "--no-ext-diff", source, "--", "."],
    )?;
    ensure!(
        diff.status.success(),
        "Tracked files differ from source or Git could not verify the checkout"
    );

    let coordinates = coordinates(root, module, version, destination, true)?;
    let Coordinates {
        group,
        artifact,
        repository_url,
        consumer_version,
    } = coordinates;
    let consumer = consumer_version.as_deref().unwrap_or(version);
    let directory = root
        .join("build/verification-repository")
        .join(group.replace('.', "/"))
        .join(&artifact)
        .join(consumer);
    let mut sha256 = BTreeMap::new();
    for suffix in SUFFIXES {
        let path = directory.join(format!("{artifact}-{consumer}{suffix}"));
        let data = fs::read(&path).with_context(|| format!("Could not read {}", path.display()))?;
        verify(&data, suffix, &group, &artifact, consumer, true)
            .with_context(|| format!("Invalid artifact {}", path.display()))?;
        sha256.insert(suffix, format!("{:x}", Sha256::digest(&data)));
    }
    Ok(Record {
        module,
        version,
        source,
        group,
        artifact,
        sha256,
        phase: "reserved-before-upload",
        workflow_run,
        repository: destination,
        repository_url,
        source_tag: format!("{module}/v{version}-{destination}"),
        consumer_version,
    })
}

pub(super) struct Coordinates {
    pub group: String,
    pub artifact: String,
    pub repository_url: &'static str,
    pub consumer_version: Option<String>,
}

pub(super) fn coordinates(
    root: &Path,
    module: &str,
    version: &str,
    destination: &str,
    scoped: bool,
) -> Result<Coordinates> {
    coordinates_for_record(root, module, version, destination, scoped, true, None)
}

pub(super) fn coordinates_for_record(
    root: &Path,
    module: &str,
    version: &str,
    destination: &str,
    scoped: bool,
    require_configured: bool,
    record_artifact: Option<&str>,
) -> Result<Coordinates> {
    ensure!(
        Regex::new(r"\A(?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*)){2}\z")?.is_match(version),
        "Expected stable X.Y.Z"
    );
    let config: ProjectConfig = serde_yaml_ng::from_str(&fs::read_to_string(
        root.join(".project/setup/config.yaml"),
    )?)?;
    ensure!(
        config.modules.contains_key(module)
            && Regex::new(r"\A[A-Za-z0-9_-][A-Za-z0-9_.-]*\z")?.is_match(module),
        "Unknown or invalid release module"
    );
    let (owner, repo) = config
        .repository
        .split_once('/')
        .context("Expected repository owner/name")?;
    let slug_part = Regex::new(r"\A[A-Za-z0-9_.-]+\z")?;
    for part in [owner, repo] {
        ensure!(
            slug_part.is_match(part) && !matches!(part, "." | ".."),
            "Invalid repository identity"
        );
    }
    if require_configured {
        let registry =
            crate::publishing::load_registry(&root.join(".project/setup/repositories.yml"))?;
        ensure!(
            registry.contains_key(destination),
            "Unknown release repository"
        );
    }

    if destination == "jitpack" {
        ensure!(
            record_artifact.is_none_or(|artifact| artifact == repo),
            "Unexpected JitPack module/artifact identity"
        );
        return Ok(Coordinates {
            group: format!("com.github.{owner}"),
            artifact: repo.to_owned(),
            repository_url: JITPACK,
            consumer_version: Some(format!(
                "{module}~v{version}{}",
                if scoped { "-jitpack" } else { "" }
            )),
        });
    }
    let (group, artifact) = local_identity(root, module)?;
    Ok(Coordinates {
        group,
        artifact,
        repository_url: CENTRAL,
        consumer_version: None,
    })
}

pub(super) fn local_identity(root: &Path, module: &str) -> Result<(String, String)> {
    let properties = fs::read_to_string(root.join("gradle.properties"))?;
    let properties: BTreeMap<_, _> = properties
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                None
            } else {
                line.split_once('=')
            }
        })
        .collect();
    let group = *properties.get("GROUP").context("Missing GROUP")?;
    let artifact = properties
        .get(format!("POM_ARTIFACT_ID.{module}").as_str())
        .or_else(|| properties.get("POM_ARTIFACT_ID"))
        .copied()
        .unwrap_or(module);
    ensure!(
        Regex::new(r"\A[A-Za-z0-9_]+(?:\.[A-Za-z0-9_]+)+\z")?.is_match(group)
            && Regex::new(r"\A[a-z][a-z0-9-]*\z")?.is_match(artifact),
        "Unexpected module coordinates"
    );
    Ok((group.to_owned(), artifact.to_owned()))
}

pub(super) fn git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .context("Could not run Git")
}

pub(super) fn verify(
    data: &[u8],
    suffix: &str,
    group: &str,
    artifact: &str,
    version: &str,
    local: bool,
) -> Result<()> {
    if suffix == ".pom" {
        return verify_pom(data, group, artifact, version);
    }
    if suffix == ".module" {
        let metadata: serde_json::Value = serde_json::from_slice(data)?;
        let component = &metadata["component"];
        ensure!(
            ["group", "module", "version"].map(|key| component[key].as_str())
                == [Some(group), Some(artifact), Some(version)],
            "Gradle module coordinates mismatch"
        );
        return Ok(());
    }

    let mut archive = ZipArchive::new(Cursor::new(data))?;
    let mut names = HashSet::new();
    let mut classes = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        names.insert(name.clone());
        if suffix == ".aar" && name == "classes.jar" {
            entry.read_to_end(&mut classes)?;
        } else {
            io::copy(&mut entry, &mut io::sink())?;
        }
        if local && suffix == "-javadoc.jar" && !entry.is_dir() {
            ensure!(
                name.ends_with(".md")
                    || matches!(name.as_str(), "LICENSE" | "META-INF/MANIFEST.MF"),
                "Unexpected documentation archive content: {name}"
            );
        }
    }
    match suffix {
        ".aar" => {
            ensure!(
                names.contains("AndroidManifest.xml") && names.contains("classes.jar"),
                "AAR is missing manifest or classes.jar"
            );
            let mut jar = ZipArchive::new(Cursor::new(classes))?;
            let mut found = false;
            for index in 0..jar.len() {
                let mut entry = jar.by_index(index)?;
                let name = entry.name();
                if name.ends_with(".class")
                    && name.starts_with("com/apexfission/android/democommon/")
                {
                    ensure!(
                        !name.contains("DemoPreviews"),
                        "Debug fixture must not be included"
                    );
                    let mut header = [0; 8];
                    entry.read_exact(&mut header)?;
                    ensure!(
                        header[..4] == [0xca, 0xfe, 0xba, 0xbe]
                            && u16::from_be_bytes([header[6], header[7]]) == 61,
                        "Unexpected JVM bytecode target"
                    );
                    io::copy(&mut entry, &mut io::sink())?;
                    found = true;
                }
            }
            ensure!(found, "Missing library classes");
        }
        "-sources.jar" => ensure!(
            names.iter().any(|name| name.ends_with(".kt")),
            "Sources JAR has no Kotlin source"
        ),
        "-javadoc.jar" => ensure!(
            names
                .iter()
                .any(|name| name.ends_with(".md") || (!local && name.ends_with(".html"))),
            "Documentation JAR has no documentation"
        ),
        _ => unreachable!(),
    }
    Ok(())
}

fn verify_pom(data: &[u8], group: &str, artifact: &str, version: &str) -> Result<()> {
    let xml = std::str::from_utf8(data)?;
    let upper = xml.to_ascii_uppercase();
    ensure!(
        !upper.contains("<!DOCTYPE") && !upper.contains("<!ENTITY"),
        "XML DTD and entity declarations are not allowed"
    );
    let document = roxmltree::Document::parse(xml)?;
    let root = document.root_element();
    let value = |path: &str| {
        path.split('/')
            .try_fold(root, |node, tag| {
                node.children().find(|child| child.has_tag_name(tag))
            })
            .and_then(|node| node.text())
    };
    ensure!(
        [value("groupId"), value("artifactId"), value("version")]
            == [Some(group), Some(artifact), Some(version)],
        "POM coordinates/version mismatch"
    );
    ensure!(
        value("packaging") == Some("aar"),
        "Publication must be an Android AAR"
    );
    for path in [
        "name",
        "description",
        "url",
        "licenses/license/name",
        "licenses/license/url",
        "developers/developer/id",
        "developers/developer/name",
        "scm/url",
        "scm/connection",
    ] {
        ensure!(
            value(path).is_some_and(|text| !text.is_empty()),
            "Missing POM metadata: {path}"
        );
    }
    let mut exported = HashSet::new();
    if let Some(dependencies) = root
        .children()
        .find(|node| node.has_tag_name("dependencies"))
    {
        for dep in dependencies
            .children()
            .filter(|node| node.has_tag_name("dependency"))
        {
            let text = |key| {
                dep.children()
                    .find(|node| node.has_tag_name(key))
                    .and_then(|node| node.text())
            };
            ensure!(
                text("artifactId") != Some("unspecified") && text("version") != Some("unspecified"),
                "Unresolved project dependency in POM"
            );
            if text("scope").unwrap_or("compile") == "compile" {
                if let Some(id) = text("artifactId") {
                    exported.insert(id);
                }
            }
        }
    }
    ensure!(
        exported.contains("foundation") && exported.contains("ui"),
        "Compose Foundation and UI must be exported"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_coordinates_follow_the_gradle_artifact_precedence() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let properties = root.join("gradle.properties");
        fs::write(
            &properties,
            "GROUP=com.example.android\nPOM_ARTIFACT_ID=shared\nPOM_ARTIFACT_ID.demo=custom-demo\n",
        )
        .unwrap();
        assert_eq!(local_identity(root, "demo").unwrap().1, "custom-demo");
        assert_eq!(local_identity(root, "other").unwrap().1, "shared");
        fs::write(&properties, "GROUP=com.example.android\n").unwrap();
        assert_eq!(local_identity(root, "demo").unwrap().1, "demo");
    }
}
