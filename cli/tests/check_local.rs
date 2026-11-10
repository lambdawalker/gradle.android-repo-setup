use serde_json::{json, Value};
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;
use zip::{write::SimpleFileOptions, ZipWriter};

const SHA_PLACEHOLDER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        writer
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

struct Candidate {
    directory: TempDir,
    source: String,
    artifact_base: PathBuf,
}

impl Candidate {
    fn create(jitpack: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join(".project/setup")).unwrap();
        fs::write(
            root.join(".project/setup/config.yaml"),
            include_str!("fixtures/config.yaml")
                .replace(
                    "lambdawalker/android.apexfission.demo-common",
                    "example/android-demo",
                )
                .replace("demo-common: Demo Common", "demo: Demo"),
        )
        .unwrap();
        fs::write(root.join(".project/setup/repositories.yml"), "repositories:\n  maven-central: {environment: maven-central, publisher: central}\n  jitpack: {environment: jitpack, publisher: jitpack}\n").unwrap();
        fs::write(
            root.join("gradle.properties"),
            "GROUP=com.example.android\nPOM_ARTIFACT_ID=demo\n",
        )
        .unwrap();
        git(root, &["init"]);
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "Fixture",
            ],
        );
        let source = git(root, &["rev-parse", "HEAD"]);
        let (group, artifact, version) = if jitpack {
            ("com.github.example", "android-demo", "demo~v9.8.7-jitpack")
        } else {
            ("com.example.android", "demo", "9.8.7")
        };
        let output = root
            .join("build/verification-repository")
            .join(group.replace('.', "/"))
            .join(artifact)
            .join(version);
        fs::create_dir_all(&output).unwrap();
        let artifact_base = output.join(format!("{artifact}-{version}"));
        let write = |suffix, bytes: &[u8]| {
            fs::write(
                artifact_base.with_file_name(format!("{artifact}-{version}{suffix}")),
                bytes,
            )
            .unwrap()
        };
        let pom = format!("<project xmlns=\"http://maven.apache.org/POM/4.0.0\"><groupId>{group}</groupId><artifactId>{artifact}</artifactId><version>{version}</version><packaging>aar</packaging><name>Demo</name><description>Fixture</description><url>https://example.invalid</url><licenses><license><name>Apache-2.0</name><url>https://example.invalid/license</url></license></licenses><developers><developer><id>test</id><name>Test</name></developer></developers><scm><url>https://example.invalid</url><connection>scm:git:https://example.invalid</connection></scm><dependencies><dependency><artifactId>foundation</artifactId></dependency><dependency><artifactId>ui</artifactId><scope>compile</scope></dependency></dependencies></project>");
        write(".pom", pom.as_bytes());
        write(
            ".module",
            json!({"component": {"group": group, "module": artifact, "version": version}})
                .to_string()
                .as_bytes(),
        );
        let classes = archive(&[(
            "com/apexfission/android/democommon/Demo.class",
            &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61],
        )]);
        write(
            ".aar",
            &archive(&[
                ("AndroidManifest.xml", b"<manifest/>"),
                ("classes.jar", &classes),
            ]),
        );
        write("-sources.jar", &archive(&[("Demo.kt", b"class Demo")]));
        write("-javadoc.jar", &archive(&[("README.md", b"# Demo")]));
        Self {
            directory,
            source,
            artifact_base,
        }
    }

    fn run(&self, destination: &str, source: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
            .arg("--project")
            .arg(self.directory.path())
            .args([
                "module_release",
                "check-local",
                "--module",
                "demo",
                "--version",
                "9.8.7",
                "--source",
                source,
            ])
            .env("RELEASE_REPOSITORY", destination)
            .env_remove("GITHUB_RUN_ID")
            .output()
            .unwrap()
    }

    fn record(&self, destination: &str) -> Value {
        let output = self.run(destination, &self.source);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn verifies_central_and_jitpack_without_writing_release_tags() {
    for (jitpack, destination) in [(false, "maven-central"), (true, "jitpack")] {
        let candidate = Candidate::create(jitpack);
        let record = candidate.record(destination);
        assert_eq!(record["source"], candidate.source);
        assert_eq!(record["repository"], destination);
        assert_eq!(record["source_tag"], format!("demo/v9.8.7-{destination}"));
        assert_eq!(record["sha256"].as_object().unwrap().len(), 5);
        assert_eq!(
            record["consumer_version"].as_str(),
            jitpack.then_some("demo~v9.8.7-jitpack")
        );
        assert!(git(candidate.directory.path(), &["tag", "--list"]).is_empty());
    }
}

#[test]
fn rejects_wrong_source_dirty_checkout_and_invalid_artifacts() {
    let candidate = Candidate::create(false);
    let fail = |source, expected: &str| {
        let result = candidate.run("maven-central", source);
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains(expected), "{error}");
    };
    fail(SHA_PLACEHOLDER, "Checkout differs from source");
    let docs = candidate
        .artifact_base
        .with_file_name("demo-9.8.7-javadoc.jar");
    let original = fs::read(&docs).unwrap();
    fs::write(
        &docs,
        archive(&[("README.md", b"# Demo"), ("image.png", b"unexpected")]),
    )
    .unwrap();
    fail(
        &candidate.source,
        "Unexpected documentation archive content",
    );
    fs::write(&docs, original).unwrap();
    fs::write(
        candidate.directory.path().join("gradle.properties"),
        "dirty",
    )
    .unwrap();
    fail(&candidate.source, "Tracked files differ from source");
}

#[test]
fn confirmation_checks_the_annotated_remote_reservation_before_network_requests() {
    let candidate = Candidate::create(false);
    let root = candidate.directory.path();
    let record = candidate.record("maven-central");
    let remote = tempfile::tempdir().unwrap();
    let remote_path = remote.path().to_str().unwrap();
    git(remote.path(), &["init", "--bare"]);
    git(root, &["remote", "add", "origin", remote_path]);
    let tag = "release-pending/demo/9.8.7";
    let reference = format!("refs/tags/{tag}");
    let run = |source: &str| {
        Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
            .arg("--project")
            .arg(root)
            .args([
                "module_release",
                "confirm",
                "--module",
                "demo",
                "--version",
                "9.8.7",
                "--source",
                source,
                "--timeout",
                "0",
            ])
            .env("RELEASE_REPOSITORY", "maven-central")
            .output()
            .unwrap()
    };
    let assert_error = |output: Output, expected: &str| {
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(expected), "{error}");
        assert!(!root.join("build/confirmed-demo.json").exists());
    };

    git(root, &["tag", tag]);
    assert_error(run(&candidate.source), "Journal must be annotated");
    git(root, &["tag", "-d", tag]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "tag.gpgsign=false",
            "tag",
            "-a",
            tag,
            "-m",
            &record.to_string(),
        ],
    );
    assert_error(run(&candidate.source), "Remote reservation differs");
    git(root, &["push", "origin", &reference]);
    assert_error(
        run(SHA_PLACEHOLDER),
        "Confirmation source differs from reservation",
    );
}
