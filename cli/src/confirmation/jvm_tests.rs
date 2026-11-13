use super::*;
use crate::release::{Packaging, VerificationConfig};
use std::collections::BTreeMap;
use std::io::Cursor;
use zip::{write::SimpleFileOptions, ZipWriter};

fn jar(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn fixture() -> (VerificationConfig, Value, [Vec<u8>; 5]) {
    let contract: VerificationConfig = serde_yaml_ng::from_str("packaging: jar\nclass_prefix: com/example/math/\nbytecode_major: 55\njvm_version: 11\nrequired_api_artifacts: []\nrequire_kotlin_stdlib: true\nforbidden_dependency_group_prefixes: [androidx.compose]\n").unwrap();
    let version = "coordinates~v0.1.1";
    let metadata = json!({"component":{"group":"com.github.example","module":"coordinates","version":version},"variants":(["java-api","java-runtime"].map(|usage|json!({"attributes":{"org.gradle.category":"library","org.gradle.usage":usage,"org.gradle.jvm.version":11,"org.jetbrains.kotlin.platform.type":"jvm"},"dependencies":[{"group":"org.jetbrains.kotlin","module":"kotlin-stdlib"}]})))});
    let pom = format!("<project><groupId>com.github.example</groupId><artifactId>coordinates</artifactId><version>{version}</version><name>Coordinates</name><description>Math</description><url>https://example.invalid</url><licenses><license><name>Apache</name><url>https://example.invalid/license</url></license></licenses><developers><developer><id>owner</id><name>Owner</name></developer></developers><scm><url>https://example.invalid</url><connection>scm:git:example</connection></scm><dependencies><dependency><groupId>org.jetbrains.kotlin</groupId><artifactId>kotlin-stdlib</artifactId></dependency></dependencies></project>").into_bytes();
    let artifacts = [
        pom,
        jar(&[(
            "com/example/math/Point.class",
            &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 55],
        )]),
        jar(&[("com/example/math/Point.kt", b"class Point")]),
        jar(&[("guide.md", b"Geometry")]),
        serde_json::to_vec(&metadata).unwrap(),
    ];
    let hashes: BTreeMap<_, _> = contract
        .suffixes()
        .into_iter()
        .zip(&artifacts)
        .map(|(suffix, bytes)| (suffix, format!("{:x}", Sha256::digest(bytes))))
        .collect();
    let content = json!({"-sources.jar":crate::release::jar_contents(&artifacts[2]).unwrap(),"-javadoc.jar":crate::release::jar_contents(&artifacts[3]).unwrap()});
    let record = json!({"module":"coordinates","group":"com.github.example","artifact":"coordinates","version":"0.1.1","consumer_version":version,"source":"a".repeat(40),"repository":"jitpack","repository_url":"https://jitpack.io","phase":"reserved-before-upload","sha256":hashes,"content_sha256":content});
    (contract, record, artifacts)
}

#[test]
fn jar_journals_retain_legacy_hashes_and_reject_mixed_or_conflicting_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let (contract, mut record, _) = fixture();
    let original = record.clone();
    assert_eq!(
        publication_suffixes(root.path(), &record).unwrap(),
        contract.suffixes()
    );
    assert_eq!(record, original);
    record["sha256"][".aar"] = json!("b".repeat(64));
    assert!(publication_suffixes(root.path(), &record).is_err());
    record = original;
    record["verification_schema"] = json!(1);
    record["verification"] = serde_json::to_value(VerificationConfig::default()).unwrap();
    assert!(publication_suffixes(root.path(), &record).is_err());
    record["verification"] = serde_json::to_value(&contract).unwrap();
    assert_eq!(
        publication_suffixes(root.path(), &record).unwrap(),
        contract.suffixes()
    );
    record["content_sha256"]
        .as_object_mut()
        .unwrap()
        .remove("-sources.jar");
    assert!(publication_suffixes(root.path(), &record).is_err());
}

#[test]
fn jitpack_jar_confirmation_checks_exact_source_archive_contents_and_optional_module() {
    let (contract, record, artifacts) = fixture();
    assert_eq!(contract.packaging, Packaging::Jar);
    let verify = |changed: bool, source: &str| {
        verify_public(
            &record,
            "jitpack",
            "https://example.invalid",
            &|url, optional| {
                if url.contains("/api/builds/") {
                    return Ok(Some(
                        serde_json::to_vec(&json!({"status":"ok","commit":source})).unwrap(),
                    ));
                }
                if optional && url.ends_with(".module") {
                    return Ok(None);
                }
                if changed && url.ends_with("-sources.jar") {
                    return Ok(Some(jar(&[(
                        "com/example/math/Point.kt",
                        b"changed source",
                    )])));
                }
                let index = contract
                    .suffixes()
                    .iter()
                    .enumerate()
                    .filter(|(_, suffix)| url.ends_with(**suffix))
                    .max_by_key(|(_, suffix)| suffix.len())
                    .unwrap()
                    .0;
                Ok(Some(artifacts[index].clone()))
            },
            &contract,
        )
    };
    assert_eq!(
        verify(false, &"a".repeat(40))
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        4
    );
    assert!(verify(true, &"a".repeat(40))
        .unwrap_err()
        .to_string()
        .contains("content differs"));
    assert!(verify(false, &"b".repeat(40))
        .unwrap_err()
        .to_string()
        .contains("reserved source"));
}

#[test]
fn legacy_jvm_confirmation_never_substitutes_current_verification_for_old_source() {
    let root = tempfile::tempdir().unwrap();
    let (_, record, _) = fixture();
    fs::create_dir_all(root.path().join(".project/setup")).unwrap();
    fs::write(
        root.path().join(".project/setup/config.yaml"),
        "verification:\n  packaging: jar\n",
    )
    .unwrap();
    assert!(crate::release::reserved_verification(root.path(), &record).is_err());
}
