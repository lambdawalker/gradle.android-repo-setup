use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_api_dependency_rejects_runtime_and_wrong_versions() {
        let contract: VerificationConfig = serde_yaml_ng::from_str("required_api_artifacts: []\nrequired_api_dependencies:\n  - group: com.example\n    artifact: core\n    version: 1.2.3\n").unwrap();
        let metadata = |usage: &str, version: &str| serde_json::json!({"variants":[{"attributes":{"org.gradle.usage":usage},"dependencies":[{"group":"com.example","module":"core","version":{"requires":version}}]}]});
        assert!(verify_module_dependencies(&metadata("java-api", "1.2.3"), &contract).is_ok());
        assert!(verify_module_dependencies(&metadata("java-runtime", "1.2.3"), &contract).is_err());
        assert!(verify_module_dependencies(&metadata("java-api", "1.2.4"), &contract).is_err());
    }
    #[test]
    fn missing_published_dependency_blocks_release() {
        let contract: VerificationConfig = serde_yaml_ng::from_str("required_api_dependencies:\n  - group: com.example\n    artifact: core\n    version: 1.2.3\n").unwrap();
        assert!(verify_published_dependencies(&contract, &|_, _| Ok(None)).is_err());
    }
    #[test]
    fn published_dependencies_follow_pom_packaging() -> Result<()> {
        use std::io::Write;
        let contract: VerificationConfig = serde_yaml_ng::from_str("required_api_dependencies:\n  - group: com.example\n    artifact: core\n    version: 1.2.3\n")?;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("content", zip::write::SimpleFileOptions::default())?;
        zip.write_all(b"content")?;
        let archive = zip.finish()?.into_inner();
        for (packaging, suffix) in [
            ("", ".jar"),
            ("<packaging>jar</packaging>", ".jar"),
            ("<packaging>aar</packaging>", ".aar"),
        ] {
            let pom = format!("<project><groupId>com.example</groupId><artifactId>core</artifactId><version>1.2.3</version>{packaging}</project>").into_bytes();
            let fetch = |url: &str, _: bool| {
                Ok(if url.ends_with(".pom") {
                    Some(pom.clone())
                } else if url.ends_with(suffix) {
                    Some(archive.clone())
                } else {
                    None
                })
            };
            assert!(
                verify_published_dependencies(&contract, &fetch).is_ok(),
                "{packaging}"
            );
            assert!(
                verify_published_dependencies(&contract, &|url: &str, _: bool| Ok(
                    if url.ends_with(".pom") {
                        Some(pom.clone())
                    } else {
                        None
                    }
                ))
                .is_err()
            );
        }
        let pom = b"<project><groupId>com.example</groupId><artifactId>core</artifactId><version>1.2.3</version><packaging>pom</packaging></project>";
        assert!(verify_published_dependencies(&contract, &|_, _| Ok(Some(pom.to_vec()))).is_err());
        Ok(())
    }
    #[test]
    fn module_assets_and_properties_come_from_source() -> Result<()> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        fs::create_dir_all(root.join(".project/setup"))?;
        fs::create_dir_all(root.join("models/src/main/assets/nested"))?;
        fs::write(
            root.join("models/src/main/assets/nested/model.tflite"),
            b"immutable model",
        )?;
        fs::write(
            root.join("gradle.properties"),
            "GROUP=com.example\ncoreArtifact=core\ncoreVersion=1.2.3\n",
        )?;
        fs::write(root.join(".project/setup/config.yaml"), "verification:\n  class_prefix: fallback/pkg/\nmodule_verification:\n  models:\n    class_prefix: models/pkg/\n    required_api_artifacts: []\n    required_api_dependencies:\n      - group_property: GROUP\n        artifact_property: coreArtifact\n        version_property: coreVersion\n    required_assets:\n      directory: models/src/main/assets\n      suffix: .tflite\n")?;
        for args in [
            vec!["init"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "fixture",
            ],
        ] {
            ensure!(git(root, &args)?.status.success(), "Fixture command failed");
        }
        fs::write(root.join("gradle.properties"), "GROUP=wrong.group\n")?;
        fs::write(
            root.join("models/src/main/assets/nested/model.tflite"),
            b"modified model",
        )?;
        let contract = verification_config(root, "HEAD", "models")?;
        assert_eq!(contract.class_prefix, "models/pkg/");
        assert_eq!(contract.required_api_dependencies[0].version, "1.2.3");
        assert_eq!(
            contract.asset_sha256["assets/nested/model.tflite"],
            format!("{:x}", Sha256::digest(b"immutable model"))
        );
        assert_eq!(
            verification_config(root, "HEAD", "other")?.class_prefix,
            "fallback/pkg/"
        );
        Ok(())
    }

    #[test]
    fn exact_pom_pin_requires_compile_scope_and_version() {
        let contract: VerificationConfig = serde_yaml_ng::from_str("required_api_dependencies:\n  - group: com.example\n    artifact: core\n    version: 1.2.3\n").unwrap();
        for (scope, version, success) in [
            ("compile", "1.2.3", true),
            ("runtime", "1.2.3", false),
            ("compile", "1.2.4", false),
        ] {
            let xml = format!("<project><dependencies><dependency><groupId>com.example</groupId><artifactId>core</artifactId><version>{version}</version><scope>{scope}</scope></dependency></dependencies></project>");
            let document = roxmltree::Document::parse(&xml).unwrap();
            assert_eq!(
                verify_pom_dependencies(document.root_element(), &contract).is_ok(),
                success
            );
        }
    }

    #[test]
    fn aar_assets_enforce_presence_hash_and_forbidden_suffix() -> Result<()> {
        use std::io::Write;
        fn zip(entries: &[(&str, &[u8])]) -> Result<Vec<u8>> {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, data) in entries {
                writer.start_file(*name, zip::write::SimpleFileOptions::default())?;
                writer.write_all(data)?;
            }
            Ok(writer.finish()?.into_inner())
        }
        let classes = zip(&[(
            "com/example/Core.class",
            &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61],
        )])?;
        let aar = |asset: Option<&[u8]>| -> Result<Vec<u8>> {
            let mut entries = vec![
                ("AndroidManifest.xml", b"manifest".as_slice()),
                ("classes.jar", classes.as_slice()),
            ];
            if let Some(asset) = asset {
                entries.push(("assets/nested/model.tflite", asset));
            }
            zip(&entries)
        };
        let mut contract = VerificationConfig {
            class_prefix: "com/example/".into(),
            ..Default::default()
        };
        contract.asset_sha256.insert(
            "assets/nested/model.tflite".into(),
            format!("{:x}", Sha256::digest(b"model")),
        );
        assert!(verify(
            &aar(Some(b"model"))?,
            ".aar",
            "g",
            "a",
            "v",
            true,
            &contract
        )
        .is_ok());
        for asset in [None, Some(b"wrong".as_slice()), Some(b"".as_slice())] {
            assert!(verify(&aar(asset)?, ".aar", "g", "a", "v", true, &contract).is_err());
        }
        contract.required_assets = Some(AssetSource {
            directory: "models/src/main/assets".into(),
            suffix: ".tflite".into(),
        });
        assert!(verify(
            &aar(Some(b"model"))?,
            ".aar",
            "g",
            "a",
            "v",
            true,
            &contract
        )
        .is_ok());
        let extra_model = zip(&[
            ("AndroidManifest.xml", b"manifest".as_slice()),
            ("classes.jar", classes.as_slice()),
            ("assets/nested/model.tflite", b"model".as_slice()),
            ("assets/unexpected.tflite", b"extra model".as_slice()),
        ])?;
        assert!(verify(&extra_model, ".aar", "g", "a", "v", true, &contract).is_err());
        contract.asset_sha256.clear();
        assert!(verify(&aar(None)?, ".aar", "g", "a", "v", true, &contract).is_err());
        contract.required_assets = None;
        contract.asset_sha256.clear();
        contract.forbidden_asset_suffixes.push(".tflite".into());
        assert!(verify(
            &aar(Some(b"model"))?,
            ".aar",
            "g",
            "a",
            "v",
            true,
            &contract
        )
        .is_err());
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DependencyPin {
    pub group: String,
    pub artifact: String,
    pub version: String,
    pub group_property: Option<String>,
    pub artifact_property: Option<String>,
    pub version_property: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssetSource {
    pub directory: String,
    pub suffix: String,
}

pub(super) fn validate_contract(contract: &VerificationConfig) -> Result<()> {
    crate::assets::validate(contract)?;
    let coordinate = Regex::new(r"\A[A-Za-z0-9_][A-Za-z0-9_.-]*\z")?;
    for pin in &contract.required_api_dependencies {
        for (value, property) in [
            (&pin.group, &pin.group_property),
            (&pin.artifact, &pin.artifact_property),
            (&pin.version, &pin.version_property),
        ] {
            if let Some(property) = property {
                ensure!(
                    value.is_empty() && !property.trim().is_empty(),
                    "Specify a dependency literal or nonempty property, not both"
                );
                continue;
            }
            ensure!(
                coordinate.is_match(value)
                    && !matches!(value.as_str(), "." | "..")
                    && !value.contains("SNAPSHOT")
                    && !matches!(
                        value.as_str(),
                        "LATEST" | "RELEASE" | "latest.release" | "latest.integration"
                    ),
                "Dependency pins require exact, stable coordinates"
            );
        }
    }
    if let Some(assets) = &contract.required_assets {
        ensure!(
            !assets.directory.starts_with('/')
                && assets
                    .directory
                    .split('/')
                    .all(|part| !matches!(part, "" | "." | ".."))
                && assets.suffix.starts_with('.')
                && assets.suffix.len() > 1,
            "Invalid required asset source"
        );
        ensure!(
            !contract.forbidden_asset_suffixes.contains(&assets.suffix),
            "Required asset suffix is forbidden"
        );
    }
    ensure!(
        contract
            .forbidden_asset_suffixes
            .iter()
            .all(|s| s.starts_with('.') && s.len() > 1),
        "Invalid forbidden asset suffix"
    );
    Ok(())
}

pub(super) fn resolve_contract(
    root: &Path,
    source: &str,
    contract: &mut VerificationConfig,
) -> Result<()> {
    if contract.required_api_dependencies.iter().any(|p| {
        p.group_property.is_some() || p.artifact_property.is_some() || p.version_property.is_some()
    }) {
        let output = git(root, &["show", &format!("{source}:gradle.properties")])?;
        ensure!(
            output.status.success(),
            "Could not read immutable gradle.properties"
        );
        let text = String::from_utf8(output.stdout)?;
        let properties: BTreeMap<_, _> = text
            .lines()
            .filter(|line| !line.trim().starts_with('#'))
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.trim(), v.trim()))
            .collect();
        for pin in &mut contract.required_api_dependencies {
            for (value, property) in [
                (&mut pin.group, &mut pin.group_property),
                (&mut pin.artifact, &mut pin.artifact_property),
                (&mut pin.version, &mut pin.version_property),
            ] {
                if let Some(property) = property.take() {
                    ensure!(
                        value.is_empty(),
                        "Specify a dependency literal or property, not both"
                    );
                    *value = properties
                        .get(property.as_str())
                        .with_context(|| format!("Missing dependency property {property}"))?
                        .to_string();
                }
            }
        }
    }
    crate::assets::resolve(root, source, contract)?;
    Ok(())
}

fn verify_jvm_variants(metadata: &serde_json::Value, contract: &VerificationConfig) -> Result<()> {
    let variants = metadata["variants"]
        .as_array()
        .context("Missing Gradle variants")?;
    let mut usages = HashSet::new();
    for variant in variants.iter().filter(|variant| {
        variant["attributes"]["org.gradle.category"] == "library"
            && matches!(
                variant["attributes"]["org.gradle.usage"].as_str(),
                Some("java-api" | "java-runtime")
            )
    }) {
        let attrs = &variant["attributes"];
        usages.insert(attrs["org.gradle.usage"].as_str().unwrap());
        ensure!(
            match attrs.get("org.jetbrains.kotlin.platform.type") {
                Some(platform) => platform == "jvm",
                None => !contract.require_kotlin_stdlib,
            },
            "Unexpected Gradle JVM platform"
        );
        ensure!(
            attrs["org.gradle.jvm.version"]
                .as_u64()
                .is_some_and(|version| version > 0
                    && contract
                        .jvm_version
                        .is_none_or(|expected| version == u64::from(expected))),
            "Unexpected Gradle JVM target"
        );
        let deps = variant["dependencies"].as_array();
        ensure!(
            !contract.require_kotlin_stdlib
                || deps.is_some_and(|deps| deps
                    .iter()
                    .any(|dep| dep["group"] == "org.jetbrains.kotlin"
                        && dep["module"] == "kotlin-stdlib")),
            "Kotlin standard library missing from Gradle variant"
        );
        if let Some(deps) = deps {
            ensure!(
                !deps.iter().any(|dep| contract
                    .forbidden_dependency_group_prefixes
                    .iter()
                    .any(|prefix| dep["group"].as_str().unwrap_or("").starts_with(prefix))),
                "Forbidden dependency group in Gradle metadata"
            );
        }
    }
    ensure!(
        usages.contains("java-api") && usages.contains("java-runtime"),
        "Missing JVM API/runtime library variants"
    );
    Ok(())
}

pub(super) fn verify_module_dependencies(
    metadata: &serde_json::Value,
    contract: &VerificationConfig,
) -> Result<()> {
    if contract.packaging == Packaging::Jar {
        verify_jvm_variants(metadata, contract)?;
    }
    if contract.required_api_dependencies.is_empty() {
        return Ok(());
    }
    let variants = metadata["variants"]
        .as_array()
        .context("Missing Gradle variants")?;
    let api: Vec<_> = variants
        .iter()
        .filter(|v| v["attributes"]["org.gradle.usage"] == "java-api")
        .collect();
    ensure!(!api.is_empty(), "Missing Gradle API variant");
    for variant in api {
        let dependencies = variant["dependencies"]
            .as_array()
            .context("Missing API dependencies")?;
        for pin in &contract.required_api_dependencies {
            let matches: Vec<_> = dependencies
                .iter()
                .filter(|d| d["group"] == pin.group && d["module"] == pin.artifact)
                .collect();
            ensure!(
                matches.len() == 1
                    && matches[0]["version"]["requires"] == pin.version
                    && matches[0]["version"]
                        .as_object()
                        .is_some_and(|v| v.iter().all(|(k, value)| k == "requires"
                            || k == "strictly" && value == &pin.version)),
                "Gradle API dependency must match exact pin: {}:{}:{}",
                pin.group,
                pin.artifact,
                pin.version
            );
        }
    }
    Ok(())
}

pub(super) fn verify_pom_dependencies(
    root: roxmltree::Node<'_, '_>,
    contract: &VerificationConfig,
) -> Result<()> {
    let dependencies: Vec<_> = root
        .children()
        .filter(|node| node.has_tag_name("dependencies"))
        .flat_map(|node| node.children())
        .filter(|node| node.has_tag_name("dependency"))
        .collect();
    let mut stdlib = false;
    for dep in dependencies {
        let text = |key| {
            dep.children()
                .find(|node| node.has_tag_name(key))
                .and_then(|node| node.text())
        };
        ensure!(
            !contract
                .forbidden_dependency_group_prefixes
                .iter()
                .any(|prefix| text("groupId").unwrap_or("").starts_with(prefix)),
            "Forbidden dependency group in POM"
        );
        if text("groupId") == Some("org.jetbrains.kotlin")
            && text("artifactId") == Some("kotlin-stdlib")
            && matches!(text("scope").unwrap_or("compile"), "compile" | "runtime")
            && text("optional").unwrap_or("false") == "false"
        {
            stdlib = true;
        }
    }
    ensure!(
        !contract.require_kotlin_stdlib || stdlib,
        "Kotlin standard library dependency missing from POM"
    );
    for pin in &contract.required_api_dependencies {
        let dependencies: Vec<_> = root
            .children()
            .filter(|n| n.has_tag_name("dependencies"))
            .flat_map(|n| n.children())
            .filter(|n| n.has_tag_name("dependency"))
            .filter(|node| {
                let text = |key| {
                    node.children()
                        .find(|n| n.has_tag_name(key))
                        .and_then(|n| n.text())
                };
                text("groupId") == Some(pin.group.as_str())
                    && text("artifactId") == Some(pin.artifact.as_str())
            })
            .collect();
        ensure!(
            dependencies.len() == 1,
            "Missing or duplicate pinned POM dependency: {}:{}",
            pin.group,
            pin.artifact
        );
        let text = |key| {
            dependencies[0]
                .children()
                .find(|n| n.has_tag_name(key))
                .and_then(|n| n.text())
        };
        ensure!(
            text("version") == Some(pin.version.as_str())
                && text("scope").unwrap_or("compile") == "compile"
                && text("optional").unwrap_or("false") == "false"
                && text("classifier").is_none()
                && text("type").is_none_or(|v| matches!(v, "aar" | "jar")),
            "POM API dependency must match exact nonoptional compile pin: {}:{}:{}",
            pin.group,
            pin.artifact,
            pin.version
        );
    }
    Ok(())
}

pub(super) fn verify_published_dependencies(
    contract: &VerificationConfig,
    fetch: &impl Fn(&str, bool) -> Result<Option<Vec<u8>>>,
) -> Result<()> {
    for pin in &contract.required_api_dependencies {
        let base = format!(
            "{}/{}/{}/{}/{}-{}",
            CENTRAL,
            pin.group.replace('.', "/"),
            pin.artifact,
            pin.version,
            pin.artifact,
            pin.version
        );
        let pom = fetch(&format!("{base}.pom"), false)?
            .with_context(|| format!("Pinned dependency POM is not published: {base}.pom"))?;
        let document = roxmltree::Document::parse(std::str::from_utf8(&pom)?)?;
        let root = document.root_element();
        let text = |key| {
            root.children()
                .find(|node| node.has_tag_name(key))
                .and_then(|node| node.text())
        };
        for (key, expected) in [
            ("groupId", &pin.group),
            ("artifactId", &pin.artifact),
            ("version", &pin.version),
        ] {
            ensure!(
                text(key) == Some(expected.as_str()),
                "Published dependency POM coordinates mismatch"
            );
        }
        let packaging = text("packaging").unwrap_or("jar");
        ensure!(
            matches!(packaging, "jar" | "aar"),
            "Unsupported published dependency packaging: {packaging}"
        );
        let url = format!("{base}.{packaging}");
        let bytes = fetch(&url, false)?
            .with_context(|| format!("Pinned dependency artifact is not published: {url}"))?;
        let mut archive = ZipArchive::new(Cursor::new(bytes))?;
        ensure!(
            !archive.is_empty(),
            "Published dependency archive is empty: {url}"
        );
        const MAX_DEPENDENCY_ARCHIVE_ENTRIES: usize = 16_384;
        const MAX_DEPENDENCY_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
        ensure!(
            archive.len() <= MAX_DEPENDENCY_ARCHIVE_ENTRIES,
            "Published dependency archive has too many entries: {url}"
        );
        let mut total_bytes = 0u64;
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            let remaining = MAX_DEPENDENCY_ARCHIVE_BYTES.saturating_sub(total_bytes);
            let mut limited = io::Read::take(entry, remaining + 1);
            let copied = io::copy(&mut limited, &mut io::sink())?;
            total_bytes = total_bytes.saturating_add(copied);
            ensure!(
                total_bytes <= MAX_DEPENDENCY_ARCHIVE_BYTES,
                "Published dependency archive exceeds maximum decompressed size: {url}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod jvm_tests {
    use super::*;
    use std::io::Write;

    fn contract() -> VerificationConfig {
        serde_yaml_ng::from_str("packaging: jar\nclass_prefix: com/example/\nbytecode_major: 55\njvm_version: 11\nrequired_api_artifacts: []\nrequire_kotlin_stdlib: true\nforbidden_dependency_group_prefixes: [androidx.compose]\n").unwrap()
    }

    fn metadata() -> serde_json::Value {
        serde_json::json!({"component":{"group":"g","module":"a","version":"v"},"variants":[
            {"attributes":{"org.gradle.category":"library","org.gradle.usage":"java-api","org.gradle.jvm.version":11,"org.jetbrains.kotlin.platform.type":"jvm"},"dependencies":[{"group":"org.jetbrains.kotlin","module":"kotlin-stdlib"}]},
            {"attributes":{"org.gradle.category":"library","org.gradle.usage":"java-runtime","org.gradle.jvm.version":11,"org.jetbrains.kotlin.platform.type":"jvm"},"dependencies":[{"group":"org.jetbrains.kotlin","module":"kotlin-stdlib"}]},
            {"attributes":{"org.gradle.category":"documentation","org.gradle.usage":"java-runtime"}}
        ]})
    }

    #[test]
    fn jvm_variants_enforce_target_platform_dependencies_and_both_usages() {
        let contract = contract();
        assert!(verify_module_dependencies(&metadata(), &contract).is_ok());
        for field in [
            "org.gradle.jvm.version",
            "org.jetbrains.kotlin.platform.type",
            "org.gradle.usage",
        ] {
            let mut invalid = metadata();
            invalid["variants"][0]["attributes"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                verify_module_dependencies(&invalid, &contract).is_err(),
                "{field}"
            );
        }
        for deps in [
            serde_json::json!([]),
            serde_json::json!([{"group":"org.jetbrains.kotlin","module":"kotlin-stdlib"},{"group":"androidx.compose.runtime","module":"runtime"}]),
        ] {
            let mut invalid = metadata();
            invalid["variants"][1]["dependencies"] = deps;
            assert!(verify_module_dependencies(&invalid, &contract).is_err());
        }
    }

    #[test]
    fn plain_java_variants_allow_absent_kotlin_platform_but_reject_wrong_platforms() {
        let mut contract = contract();
        contract.require_kotlin_stdlib = false;
        let mut java = metadata();
        for variant in java["variants"].as_array_mut().unwrap().iter_mut().take(2) {
            variant["attributes"]
                .as_object_mut()
                .unwrap()
                .remove("org.jetbrains.kotlin.platform.type");
            variant.as_object_mut().unwrap().remove("dependencies");
        }
        assert!(verify_module_dependencies(&java, &contract).is_ok());
        contract.require_kotlin_stdlib = true;
        assert!(verify_module_dependencies(&java, &contract).is_err());
        contract.require_kotlin_stdlib = false;
        for platform in [
            serde_json::json!("native"),
            serde_json::json!("androidJvm"),
            serde_json::json!(null),
        ] {
            java["variants"][0]["attributes"]["org.jetbrains.kotlin.platform.type"] = platform;
            assert!(verify_module_dependencies(&java, &contract).is_err());
        }
    }

    #[test]
    fn jar_checks_classes_bytecode_crc_and_primary_extension() -> Result<()> {
        let contract = contract();
        assert_eq!(contract.suffixes()[1], ".jar");
        for (name, major, valid) in [
            ("com/example/Math.class", 55, true),
            ("com/other/Math.class", 55, false),
            ("com/example/Math.class", 52, false),
        ] {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            writer.start_file(name, zip::write::SimpleFileOptions::default())?;
            writer.write_all(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, major])?;
            let bytes = writer.finish()?.into_inner();
            assert_eq!(
                verify(&bytes, ".jar", "g", "a", "v", true, &contract).is_ok(),
                valid
            );
            assert!(verify(&bytes, ".aar", "g", "a", "v", true, &contract).is_err());
        }
        Ok(())
    }

    #[test]
    fn jar_contents_ignore_manifest_and_reject_duplicate_entries() -> Result<()> {
        let build = |manifest: &[u8]| -> Result<Vec<u8>> {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            writer.start_file(
                "META-INF/MANIFEST.MF",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )?;
            writer.write_all(manifest)?;
            writer.start_file("source.kt", zip::write::SimpleFileOptions::default())?;
            writer.write_all(b"source")?;
            Ok(writer.finish()?.into_inner())
        };
        assert_eq!(
            jar_contents(&build(b"one")?)?,
            jar_contents(&build(b"two")?)?
        );
        assert_eq!(jar_contents(&build(b"one")?)?.len(), 1);
        // Mutate the manifest payload without changing its stored CRC.
        let mut corrupt = build(b"unique-manifest")?;
        let offset = corrupt
            .windows(b"unique-manifest".len())
            .position(|part| part == b"unique-manifest")
            .unwrap();
        corrupt[offset] ^= 1;
        assert!(jar_contents(&corrupt).is_err());
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for name in ["a.kt", "b.kt"] {
            writer.start_file(name, zip::write::SimpleFileOptions::default())?;
            writer.write_all(b"source")?;
        }
        let mut duplicate = writer.finish()?.into_inner();
        for offset in 0..duplicate.len() - 3 {
            if &duplicate[offset..offset + 4] == b"b.kt" {
                duplicate[offset] = b'a';
            }
        }
        assert!(jar_contents(&duplicate).is_err());
        Ok(())
    }

    #[test]
    fn java_only_sources_are_accepted_for_jvm_but_not_aar() -> Result<()> {
        for name in ["Geometry.java", "com/example/Geometry.java"] {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            writer.start_file(name, zip::write::SimpleFileOptions::default())?;
            writer.write_all(b"class Geometry {}")?;
            let bytes = writer.finish()?.into_inner();
            assert!(verify(&bytes, "-sources.jar", "g", "a", "v", true, &contract()).is_ok());
            assert!(verify(
                &bytes,
                "-sources.jar",
                "g",
                "a",
                "v",
                true,
                &VerificationConfig::default()
            )
            .is_err());
        }
        Ok(())
    }

    #[test]
    fn legacy_snapshots_remain_aar_without_jvm_fields() -> Result<()> {
        let contract = VerificationConfig::default();
        let mut snapshot = serde_json::to_value(&contract)?;
        for key in [
            "packaging",
            "jvm_version",
            "require_kotlin_stdlib",
            "forbidden_dependency_group_prefixes",
        ] {
            snapshot.as_object_mut().unwrap().remove(key);
        }
        let record = serde_json::json!({"verification_schema":1,"verification":snapshot});
        assert_eq!(
            reserved_verification(Path::new("."), &record)?.suffixes()[1],
            ".aar"
        );
        Ok(())
    }

    #[test]
    fn pom_jvm_policy_checks_packaging_stdlib_and_compose() -> Result<()> {
        let contract = contract();
        for (deps, valid) in [
            ("<dependency><groupId>org.jetbrains.kotlin</groupId><artifactId>kotlin-stdlib</artifactId></dependency>",true),
            ("",false),
            ("<dependency><groupId>org.jetbrains.kotlin</groupId><artifactId>kotlin-stdlib</artifactId></dependency><dependency><groupId>androidx.compose.runtime</groupId><artifactId>runtime</artifactId></dependency>",false),
            ("<dependency><groupId>org.jetbrains.kotlin</groupId><artifactId>kotlin-stdlib</artifactId><scope>test</scope></dependency>",false)
        ] {
            let xml = format!("<project><dependencies>{deps}</dependencies></project>");
            let document = roxmltree::Document::parse(&xml)?;
            assert_eq!(verify_pom_dependencies(document.root_element(),&contract).is_ok(),valid);
        }
        Ok(())
    }
}
