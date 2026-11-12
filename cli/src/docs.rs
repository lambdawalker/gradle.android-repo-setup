mod history;
mod site;
mod zola;

pub(crate) use history::{archive_record, history, seed, validate_archive, HISTORY};
pub(crate) fn build(root: &Path, base: Option<&str>, check: bool) -> Result<()> {
    if root.join("sites/zola.toml").is_file() {
        zola::build(root, base, check)
    } else {
        site::build(root, base)
    }
}

use anyhow::{anyhow, ensure, Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

const CENTRAL: &str = "https://repo.maven.apache.org/maven2";
const GOOGLE: &str = "https://dl.google.com/dl/android/maven2";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default, rename = "verification")]
    _verification: Option<crate::release::VerificationConfig>,
    #[serde(default, rename = "android_modules")]
    _android_modules: Option<crate::discovery::Modules>,
    #[serde(default, rename = "module_verification")]
    _module_verification: std::collections::BTreeMap<String, crate::release::VerificationConfig>,
    #[serde(default, rename = "fingerprint_excluded_properties")]
    _fingerprint_excluded_properties: std::collections::BTreeMap<String, Vec<String>>,
    repository: String,
    title: String,
    modules: serde_yaml_ng::Mapping,
    tooling_ref: String,
    #[serde(default, rename = "workflows")]
    _workflows: Option<serde_yaml_ng::Value>,
    #[serde(default, rename = "key_alias")]
    _key_alias: Option<serde_yaml_ng::Value>,
    #[serde(default, rename = "signing")]
    _signing: Option<serde_yaml_ng::Value>,
}

impl Config {
    fn load(root: &Path) -> Result<Self> {
        let config: Self =
            serde_yaml_ng::from_str(&read(root.join(".project/setup/config.yaml"))?)?;
        ensure!(
            matches(
                r"[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9_.-]+",
                &config.repository
            ) && !matches!(config.repository.split('/').next_back(), Some("." | "..")),
            "Repository must have owner/name form"
        );
        ensure!(
            matches(
                r"(?:v[0-9]+\.[0-9]+\.[0-9]+|[0-9a-f]{40})",
                &config.tooling_ref
            ),
            "tooling_ref must be a release tag or full commit SHA"
        );
        for (name, label) in &config.modules {
            ensure!(
                name.as_str()
                    .is_some_and(|name| matches(r"[A-Za-z0-9_-][A-Za-z0-9_.-]*", name))
                    && label.is_string(),
                "Invalid documentation module configuration"
            );
        }
        Ok(config)
    }

    fn modules(&self) -> impl Iterator<Item = (&str, &str)> {
        self.modules
            .iter()
            .map(|(name, label)| (name.as_str().unwrap(), label.as_str().unwrap()))
    }
}

fn read(path: PathBuf) -> Result<String> {
    Ok(fs::read_to_string(&path)
        .with_context(|| format!("Could not read {}", path.display()))?
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}

fn read_json(path: PathBuf) -> Result<Value> {
    serde_json::from_str(&read(path.clone())?)
        .with_context(|| format!("Invalid JSON in {}", path.display()))
}

fn matches(pattern: &str, value: &str) -> bool {
    Regex::new(&format!(r"\A(?:{pattern})\z"))
        .unwrap()
        .is_match(value)
}

fn string<'a>(record: &'a Value, key: &str) -> Result<&'a str> {
    record[key]
        .as_str()
        .ok_or_else(|| anyhow!("Invalid installation {key}"))
}

fn string_or<'a>(record: &'a Value, key: &str, default: &'a str) -> Result<&'a str> {
    if record.get(key).is_some() {
        string(record, key)
    } else {
        Ok(default)
    }
}

fn coordinate(group: &str, artifact: &str) -> Result<()> {
    ensure!(
        matches(r"[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)+", group),
        "Invalid installation group"
    );
    ensure!(
        matches(r"[A-Za-z0-9_][A-Za-z0-9_.-]*", artifact),
        "Invalid installation artifact"
    );
    Ok(())
}

type Version<'a> = [(usize, &'a str); 3];

fn version_key(value: &str) -> Result<Version<'_>> {
    ensure!(
        matches(
            r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)",
            value
        ),
        "Invalid installation version"
    );
    let mut components = value.split('.');
    Ok(std::array::from_fn(|_| {
        let component = components.next().unwrap();
        (component.len(), component)
    }))
}

fn repository_url(value: &str) -> Result<&str> {
    ensure!(
        matches(r"https://[A-Za-z0-9._:/%+~-]+", value),
        "Invalid installation repository URL"
    );
    let authority = value["https://".len()..].split('/').next().unwrap();
    let (host, port) = authority.split_once(':').unwrap_or((authority, ""));
    let significant_port = port.trim_start_matches('0');
    ensure!(
        !host.is_empty()
            && (significant_port.is_empty()
                || significant_port.bytes().all(|byte| byte.is_ascii_digit())
                    && significant_port.parse::<u16>().is_ok()),
        "Invalid installation repository URL"
    );
    Ok(value.trim_end_matches('/'))
}

pub(crate) fn validate_record(target: &str, module: &str, record: &Value) -> Result<()> {
    ensure!(
        record["module"] == module && string_or(record, "repository", "maven-central")? == target,
        "Installation metadata module/repository mismatch"
    );
    ensure!(
        matches(r"[a-z][a-z0-9-]{0,62}", target),
        "Invalid installation repository"
    );
    coordinate(string(record, "group")?, string(record, "artifact")?)?;
    let version = string(record, "version")?;
    version_key(version)?;
    ensure!(
        matches("[0-9a-f]{40}", string(record, "source")?),
        "Invalid installation source"
    );
    if let Some(identity) = record.get("release_identity") {
        ensure!(
            identity.is_object()
                && identity["schema"] == 1
                && ["module", "version", "source"]
                    .iter()
                    .all(|key| identity[key] == record[key])
                && identity["fingerprint"]
                    .as_str()
                    .is_some_and(|value| matches("[0-9a-f]{64}", value)),
            "Invalid installation release identity"
        );
    }
    let consumer = string_or(record, "consumer_version", version)?;
    ensure!(
        matches("[A-Za-z0-9_][A-Za-z0-9_.~+-]*", consumer),
        "Invalid installation consumer version"
    );
    repository_url(string_or(record, "repository_url", CENTRAL)?)?;
    let scoped = record.get("source_tag").is_some();
    if scoped {
        let legacy = &record["legacy_tag"];
        ensure!(
            record["source_tag"] == format!("{module}/v{version}-{target}")
                && (matches!(legacy, Value::Null | Value::Bool(false))
                    || legacy.as_str().is_some_and(str::is_empty)
                    || legacy.as_f64() == Some(0.0)
                    || legacy.as_array().is_some_and(Vec::is_empty)
                    || legacy.as_object().is_some_and(serde_json::Map::is_empty)),
            "Invalid destination source tag"
        );
    }
    if target == "jitpack" {
        let suffix = if scoped { "-jitpack" } else { "" };
        ensure!(
            consumer == format!("{module}~v{version}{suffix}"),
            "JitPack consumer version must match the module release"
        );
    }
    Ok(())
}

fn examples(module: &str, record: &Value) -> Result<String> {
    let group = string(record, "group")?;
    let artifact = string(record, "artifact")?;
    let version = string_or(record, "consumer_version", string(record, "version")?)?;
    let gav = format!("{group}:{artifact}:{version}");
    let extra = repository_url(string_or(record, "repository_url", CENTRAL)?)?;
    let mut urls = vec![GOOGLE, CENTRAL];
    let (kotlin_repos, groovy_repos) = if urls.contains(&extra) {
        (String::new(), String::new())
    } else {
        urls.push(extra);
        (
            format!("\n        maven {{ url = uri(\"{extra}\") }}"),
            format!("\n        maven {{ url '{extra}' }}"),
        )
    };
    let maven_repos = urls
        .iter()
        .zip(["google", "central", "confirmed-2"])
        .map(|(url, id)| {
            format!("  <repository>\n    <id>{id}</id>\n    <url>{url}</url>\n  </repository>")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let catalog_module = module.replace('-', ".");
    Ok(format!(
        r#"#### Gradle Kotlin DSL

In `settings.gradle.kts`:

```kotlin
dependencyResolutionManagement {{
    repositories {{
        google()
        mavenCentral(){kotlin_repos}
    }}
}}
```

In the app's `build.gradle.kts`:

```kotlin
dependencies {{
    implementation("{gav}")
}}
```

#### Gradle Groovy DSL

In `settings.gradle`:

```groovy
dependencyResolutionManagement {{
    repositories {{
        google()
        mavenCentral(){groovy_repos}
    }}
}}
```

In the app's `build.gradle`:

```groovy
dependencies {{
    implementation '{gav}'
}}
```

#### Version catalog

Use the dependency repositories shown above. Add to `gradle/libs.versions.toml`:

```toml
[libraries]
{module} = {{ module = "{group}:{artifact}", version = "{version}" }}
```

Then use this instead of the direct dependency in the app's `build.gradle.kts`:

```kotlin
dependencies {{
    implementation(libs.{catalog_module})
}}
```

#### Maven

Add these repositories and dependency to `pom.xml`:

```xml
<repositories>
{maven_repos}
</repositories>
<dependencies>
  <dependency>
    <groupId>{group}</groupId>
    <artifactId>{artifact}</artifactId>
    <version>{version}</version>
    <type>aar</type>
  </dependency>
</dependencies>
```"#
    ))
}

fn render_module(
    module: &str,
    label: &str,
    configured: (&str, &str),
    records: Vec<(&str, Value)>,
    repository: &str,
) -> Result<String> {
    let mut candidates = Vec::new();
    for (target, record) in &records {
        if record.is_null() || record["phase"] == "reserved-before-upload" {
            continue;
        }
        ensure!(
            record["phase"] == "confirmed-public",
            "Installation can only advertise confirmed releases"
        );
        validate_record(target, module, record)?;
        candidates.push((*target, record, version_key(string(record, "version")?)?));
    }
    let Some(highest) = candidates.iter().map(|(_, _, key)| *key).max() else {
        return Ok(format!("## {label}\n\nNo confirmed release."));
    };
    let independent = candidates
        .iter()
        .any(|(_, record, _)| record.get("source_tag").is_some());
    if !independent {
        candidates.retain(|(_, _, key)| *key == highest);
        let sources: BTreeSet<_> = candidates
            .iter()
            .map(|(_, record, _)| record["source"].as_str().unwrap())
            .collect();
        ensure!(
            sources.len() == 1,
            "Conflicting confirmed sources for latest {module} version"
        );
    }
    candidates.sort_by_key(|(target, _, _)| (*target != "maven-central", *target));
    let (_, first, _) = candidates[0];
    let (group, artifact) = configured;
    let mut section = if independent {
        format!("## {module}: {artifact}\n\nChoose one publishing destination and one dependency syntax.")
    } else {
        let artifact = string(first, "artifact")?;
        let version = string(first, "version")?;
        let source = string(first, "source")?;
        format!(
            "## {module}: {artifact}\n\nConfirmed version: **{version}**. Source: [{source}](https://github.com/{repository}/commit/{source}).\n\nChoose **one** destination below and **one** dependency syntax. Each destination provides this same release; do not add duplicate dependencies."
        )
    };
    for (target, record, _) in candidates {
        section.push_str(&format!("\n\n### {target}\n\nRepository: **{target}**."));
        if independent {
            let source = string(record, "source")?;
            let version = string(record, "version")?;
            section.push_str(&format!(
                " Confirmed version **{version}**. [Source commit](https://github.com/{repository}/commit/{source})."
            ));
        }
        if target != "jitpack" && (record["group"] != group || record["artifact"] != artifact) {
            section.push_str(&format!(
                "\n\nConfigured next publication: `{group}:{artifact}` — not yet confirmed here. The dependency below remains the last confirmed publication."
            ));
        }
        section.push_str("\n\n");
        section.push_str(&examples(module, record)?);
    }
    Ok(section)
}

pub(crate) fn documentation(root: &Path, verify: bool) -> Result<()> {
    let config = Config::load(root)?;
    let release_dir = root.join("docs/releases");
    let mut targets = vec![("maven-central".to_string(), release_dir.clone())];
    for path in children(&release_dir)? {
        if path.is_dir() && children(&path)?.iter().any(|p| json_path(p)) {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .context("Invalid destination path")?;
            let directory = if name == "maven-central" {
                &release_dir
            } else {
                &path
            };
            targets.push((name.to_owned(), directory.to_owned()));
        }
    }
    targets.sort();
    targets.dedup();
    let mut sections = Vec::new();
    for (module, label) in config.modules() {
        let (group, artifact) = crate::release::local_identity(root, module)?;
        let mut records = Vec::new();
        for (target, folder) in &targets {
            let path = folder.join(format!("{module}.json"));
            if path.exists() {
                records.push((target.as_str(), read_json(path)?));
            }
        }
        sections.push(render_module(
            module,
            label,
            (&group, &artifact),
            records,
            &config.repository,
        )?);
    }
    let template = read(root.join("docs/templates/MODULE_IMPORT.md.template"))?;
    let expected = template.replace("{{INSTALLATION}}", &sections.join("\n\n"));
    ensure!(!expected.contains("{{"), "Unknown template placeholder");
    let path = root.join("IMPORT.md");
    if verify {
        ensure!(
            read(path)? == expected,
            "IMPORT.md drift; run ./gradlew generateImportDocs"
        );
    } else {
        fs::write(path, expected)?;
    }
    Ok(())
}

fn children(path: &Path) -> Result<Vec<PathBuf>> {
    match fs::read_dir(path) {
        Ok(entries) => {
            let mut paths = entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            paths.sort();
            Ok(paths)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("Could not list {}", path.display())),
    }
}

fn json_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".json"))
}
