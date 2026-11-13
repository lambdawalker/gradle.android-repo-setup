use android_repo_docs::media;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};
fn hash(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn png(rgb: bool) -> Vec<u8> {
    let mut b = Vec::new();
    {
        let mut e = png::Encoder::new(&mut b, 1, 1);
        e.set_color(if rgb {
            png::ColorType::Rgb
        } else {
            png::ColorType::Rgba
        });
        e.set_depth(png::BitDepth::Eight);
        let mut w = e.write_header().unwrap();
        w.write_image_data(if rgb { &[1, 2, 3] } else { &[1, 2, 3, 255] })
            .unwrap();
    }
    b
}
fn write(root: &Path, p: &str, v: &Value) {
    let p = root.join(p);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, serde_json::to_vec(v).unwrap()).unwrap();
}
fn git(root: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap()
        .success());
}
fn config(root: &Path, s: Value) {
    write(
        root,
        ".project/docs/config.yaml",
        &json!({"schema":1,"repository":"x/y","title":"Y","tooling_ref":"v1.0.0","screenshots":s}),
    );
}
#[test]
fn decoded_pixels_ignore_encoding_and_reject_truncation() {
    assert_eq!(
        media::pixels(&png(true)).unwrap(),
        media::pixels(&png(false)).unwrap()
    );
    assert!(media::pixels(&png(true)[..33]).is_err());
}
#[test]
fn input_hash_tracks_untracked_and_deleted_files() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    git(r, &["init", "-q"]);
    fs::write(r.join("input"), "a").unwrap();
    git(r, &["add", "input"]);
    let p = vec!["input".into(), "new".into()];
    let a = media::input_hash(r, &p).unwrap();
    fs::remove_file(r.join("input")).unwrap();
    let b = media::input_hash(r, &p).unwrap();
    assert_ne!(a, b);
    assert_eq!(b, hash(b"input\0<deleted>"));
    fs::write(r.join("new"), "x").unwrap();
    assert_ne!(b, media::input_hash(r, &p).unwrap());
}
#[test]
fn acceptance_prevalidates_every_candidate_and_preserves_metadata() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    git(r, &["init", "-q"]);
    fs::write(r.join("input"), "a").unwrap();
    let accepted = png(true);
    let candidate = png(false);
    fs::create_dir_all(r.join("accepted")).unwrap();
    fs::create_dir_all(r.join("candidates")).unwrap();
    fs::create_dir_all(r.join("legacy")).unwrap();
    for n in ["one", "two"] {
        fs::write(r.join(format!("accepted/{n}.png")), &accepted).unwrap();
        fs::write(r.join(format!("legacy/{n}.png")), &accepted).unwrap();
    }
    let s = json!({"manifest":"accepted/manifest.json","base":"accepted","candidates":"candidates","input_paths":["input"],"command":["renderer"],"legacy_base":"legacy"});
    config(r, s.clone());
    let manifest = json!({"custom":"historical","scenarios":[{"file":"one.png","preview":"One","alt":"one","capture_kind":"test","sha256":hash(&accepted)},{"file":"two.png","preview":"Two","alt":"two","capture_kind":"test","sha256":hash(&accepted)}]});
    write(r, "accepted/manifest.json", &manifest);
    write(
        r,
        "candidates/capture.json",
        &json!({"source_commit":"a".repeat(40),"input_sha256":media::input_hash(r,&["input".into()]).unwrap(),"config_sha256":hash(&serde_json::to_vec(&s).unwrap())}),
    );
    fs::write(r.join("candidates/One.png"), &candidate).unwrap();
    fs::write(r.join("candidates/Two.png"), b"invalid").unwrap();
    assert!(media::screenshots(r, "update", None).is_err());
    assert_eq!(fs::read(r.join("accepted/one.png")).unwrap(), accepted);
    fs::write(r.join("candidates/Two.png"), &candidate).unwrap();
    fs::write(r.join("candidates/Two_extra.png"), &candidate).unwrap();
    assert!(media::screenshots(r, "update", None).is_err());
    fs::remove_file(r.join("candidates/Two_extra.png")).unwrap();
    media::screenshots(r, "check", None).unwrap();
    media::screenshots(r, "update", None).unwrap();
    let v: Value =
        serde_json::from_slice(&fs::read(r.join("accepted/manifest.json")).unwrap()).unwrap();
    assert_eq!(v["custom"], "historical");
    assert_eq!(fs::read(r.join("legacy/one.png")).unwrap(), candidate);
    fs::write(r.join("input"), "changed").unwrap();
    assert!(media::screenshots(r, "update", None).is_err());
}
#[test]
fn accepted_only_manifest_requires_unique_entries_and_dimensions() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    let b = png(true);
    fs::write(r.join("image.png"), &b).unwrap();
    let spec = json!({"manifests":[{"path":"images.json","base":".","collection":"captures","file_key":"path","hash_key":"image_sha256","required_files":["image.png"],"require_dimensions":true}]});
    write(
        r,
        ".project/docs/config.yaml",
        &json!({"schema":1,"repository":"x/y","title":"Y","tooling_ref":"v1.0.0","media":spec}),
    );
    let entry = json!({"path":"image.png","image_sha256":hash(&b),"width":1,"height":1});
    write(r, "images.json", &json!({"captures":[entry.clone()]}));
    media::verify(r).unwrap();
    media::media(r, "verify").unwrap();
    fs::write(r.join("extra.png"), &b).unwrap();
    let mut extra = entry.clone();
    extra["path"] = json!("extra.png");
    write(r, "images.json", &json!({"captures":[entry.clone(),extra]}));
    assert!(media::verify(r)
        .unwrap_err()
        .to_string()
        .contains("unexpected"));
    write(r, "images.json", &json!({"captures":[entry.clone(),entry]}));
    assert!(media::verify(r).is_err());
    assert!(media::path(r, "../escape").is_err());
}
#[test]
fn frames_require_current_source_and_configuration_before_acceptance() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    let b = png(true);
    fs::write(r.join("source"), b"recording").unwrap();
    fs::write(r.join("accepted.png"), &b).unwrap();
    fs::write(r.join("candidate.png"), png(false)).unwrap();
    fs::write(r.join("guide"), "guide").unwrap();
    let spec = json!({"manifest":"manifest.json","base":".","required_ids":["history"],"source":{"path":"source","min_bytes":1},"frames":[{"id":"history","candidate":"candidate.png","evidence":"evidence.json","command":["ffmpeg","-ss","12"]}]});
    write(
        r,
        ".project/docs/config.yaml",
        &json!({"schema":1,"repository":"x/y","title":"Y","tooling_ref":"v1.0.0","media":spec.clone()}),
    );
    write(
        r,
        "manifest.json",
        &json!({"recording":{"sha256":hash(b"recording"),"capture_commit":null,"scope":"Historical"},"scenarios":[{"id":"history","file":"accepted.png","sha256":hash(&b),"guide":"guide"}]}),
    );
    let evidence = json!({"source_sha256":hash(b"recording"),"output_sha256":hash(&png(false)),"config_sha256":hash(&serde_json::to_vec(&spec).unwrap()),"tool":"ffmpeg","command":["ffmpeg","-ss","12"]});
    write(r, "evidence.json", &evidence);
    media::media(r, "verify").unwrap();
    media::media(r, "accept").unwrap();
    let m: Value = serde_json::from_slice(&fs::read(r.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["recording"]["scope"], "Historical");
    assert_eq!(m["recording"]["capture_commit"], Value::Null);
    assert_eq!(m["scenarios"][0]["render"]["command"], evidence["command"]);
    let mut bad = evidence.clone();
    bad["config_sha256"] = json!("stale");
    write(r, "evidence.json", &bad);
    assert!(media::media(r, "accept").is_err());
    write(r, "evidence.json", &evidence);
    fs::write(r.join("source"), "changed").unwrap();
    assert!(media::media(r, "accept").is_err());
}

fn frame_fixture(root: &Path, frames: Value) {
    fs::write(root.join("source"), b"recording").unwrap();
    fs::write(root.join("accepted.png"), png(true)).unwrap();
    write(
        root,
        "manifest.json",
        &json!({"recording":{"sha256":hash(b"recording")},"scenarios":[{"id":"history","file":"accepted.png","sha256":hash(&png(true))}]}),
    );
    write(
        root,
        ".project/docs/config.yaml",
        &json!({"schema":1,"repository":"x/y","title":"Y","tooling_ref":"v1.0.0","media":{"manifest":"manifest.json","base":".","source":{"path":"source"},"frames":frames}}),
    );
}
#[test]
fn frame_output_collisions_reject_before_any_command_or_write() {
    for destination in [
        "accepted.png",
        "source",
        "manifest.json",
        ".project/docs/config.yaml",
    ] {
        for key in ["candidate", "evidence"] {
            let t = tempfile::tempdir().unwrap();
            let r = t.path();
            let mut frame = json!({"id":"history","candidate":"candidate.png","evidence":"evidence.json","command":["command-must-not-run"]});
            frame[key] = json!(destination);
            frame_fixture(r, json!([frame]));
            let protected = [
                "accepted.png",
                "source",
                "manifest.json",
                ".project/docs/config.yaml",
            ];
            let before: Vec<_> = protected
                .iter()
                .map(|p| fs::read(r.join(p)).unwrap())
                .collect();
            let error = media::media(r, "render").unwrap_err().to_string();
            assert!(error.contains("overlaps protected"), "{error}");
            for (p, b) in protected.iter().zip(before) {
                assert_eq!(fs::read(r.join(p)).unwrap(), b);
            }
        }
    }
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    frame_fixture(
        r,
        json!([{"id":"history","candidate":"same","evidence":"same","command":["command-must-not-run"]}]),
    );
    assert!(media::media(r, "render")
        .unwrap_err()
        .to_string()
        .contains("output collision"));
}
#[test]
fn successful_frame_command_without_output_cannot_refresh_old_candidate() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    let executable = if cfg!(windows) { "cmd" } else { "true" };
    let command = if cfg!(windows) {
        json!([executable, "/C", "exit 0"])
    } else {
        json!([executable])
    };
    frame_fixture(
        r,
        json!([{"id":"history","candidate":"candidate.png","evidence":"evidence.json","command":command}]),
    );
    fs::write(r.join("candidate.png"), png(true)).unwrap();
    write(r, "evidence.json", &json!({"old":true}));
    assert!(media::media(r, "render").is_err());
    assert!(!r.join("candidate.png").exists());
    assert!(!r.join("evidence.json").exists());
    assert_eq!(fs::read(r.join("accepted.png")).unwrap(), png(true));
}
#[test]
fn screenshot_render_protects_nested_accepted_legacy_and_source_directories() {
    for (base, legacy, input, candidates, file) in [
        (
            "accepted",
            None,
            "input",
            "accepted/subset",
            "subset/one.png",
        ),
        ("accepted", None, "src", "src/generated", "one.png"),
        (
            "accepted",
            Some("legacy"),
            "input",
            "legacy/subset",
            "one.png",
        ),
        ("accepted", None, "input", ".project/docs", "one.png"),
        ("accepted", None, "src/generated", "src", "one.png"),
    ] {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        git(r, &["init", "-q"]);
        let image = format!("{base}/{file}");
        fs::create_dir_all(r.join(&image).parent().unwrap()).unwrap();
        fs::write(r.join(&image), png(true)).unwrap();
        if let Some(l) = legacy {
            let p = r.join(l).join(file);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, png(true)).unwrap();
        }
        fs::create_dir_all(r.join(input)).unwrap();
        fs::write(r.join(input).join("keep"), "keep").unwrap();
        fs::create_dir_all(r.join(candidates)).unwrap();
        fs::write(r.join(candidates).join("sentinel"), "keep").unwrap();
        write(
            r,
            "accepted/manifest.json",
            &json!({"scenarios":[{"file":file,"preview":"One","alt":"one","capture_kind":"test","sha256":hash(&png(true))}]}),
        );
        let mut spec = json!({"manifest":"accepted/manifest.json","base":base,"candidates":candidates,"input_paths":[input],"command":["command-must-not-run"]});
        if let Some(l) = legacy {
            spec["legacy_base"] = json!(l);
        }
        config(r, spec);
        let error = media::screenshots(r, "render", None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("overlaps"), "{error}");
        assert_eq!(
            fs::read(r.join(candidates).join("sentinel")).unwrap(),
            b"keep"
        );
        assert_eq!(fs::read(r.join(&image)).unwrap(), png(true));
        assert!(r.join(input).join("keep").exists());
    }
}

#[test]
fn screenshot_evidence_reports_all_differences_and_rejects_unsafe_outputs() {
    for output in [
        "build/evidence.json",
        "accepted/one.png",
        "accepted/manifest.json",
        "input",
        ".project/docs/config.yaml",
        "candidates/report.json",
    ] {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        git(r, &["init", "-q"]);
        git(
            r,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture",
            ],
        );
        fs::write(r.join("input"), "input").unwrap();
        fs::create_dir_all(r.join("accepted")).unwrap();
        fs::create_dir_all(r.join("candidates")).unwrap();
        for (name, preview) in [("one", "One"), ("two", "Two")] {
            fs::write(r.join(format!("accepted/{name}.png")), png(true)).unwrap();
            fs::write(r.join(format!("candidates/{preview}.png")), png(false)).unwrap();
        }
        let spec = json!({"manifest":"accepted/manifest.json","base":"accepted","candidates":"candidates","input_paths":["input"],"command":["renderer"],"evidence":output});
        config(r, spec.clone());
        let manifest = json!({"scenarios":[{"file":"one.png","preview":"One","alt":"one","capture_kind":"test","sha256":hash(&png(true))},{"file":"two.png","preview":"Two","alt":"two","capture_kind":"test","sha256":hash(&png(true))}]});
        write(r, "accepted/manifest.json", &manifest);
        write(
            r,
            "candidates/capture.json",
            &json!({"source_commit":"a".repeat(40),"input_sha256":media::input_hash(r,&["input".into()]).unwrap(),"config_sha256":hash(&serde_json::to_vec(&spec).unwrap())}),
        );
        if output != "build/evidence.json" {
            let before = fs::read(r.join("accepted/manifest.json")).unwrap();
            assert!(media::screenshots(r, "check", None)
                .unwrap_err()
                .to_string()
                .contains("evidence overlaps"));
            assert_eq!(fs::read(r.join("accepted/manifest.json")).unwrap(), before);
            continue;
        }
        media::screenshots(r, "check", None).unwrap();
        let report: Value = serde_json::from_slice(&fs::read(r.join(output)).unwrap()).unwrap();
        assert_eq!(report["mode"], "check");
        assert_eq!(report["compared"], json!(["one.png", "two.png"]));
        assert_eq!(report["differences"], json!([]));
        assert_eq!(report["source_commit"].as_str().unwrap().len(), 40);
        let mut different = Vec::new();
        {
            let mut e = png::Encoder::new(&mut different, 1, 1);
            e.set_color(png::ColorType::Rgb);
            e.set_depth(png::BitDepth::Eight);
            e.write_header()
                .unwrap()
                .write_image_data(&[5, 6, 7])
                .unwrap();
        }
        for preview in ["One", "Two"] {
            fs::write(r.join(format!("candidates/{preview}.png")), &different).unwrap();
        }
        let before = fs::read(r.join("accepted/manifest.json")).unwrap();
        let error = media::screenshots(r, "check", None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("one.png, two.png"));
        let report: Value = serde_json::from_slice(&fs::read(r.join(output)).unwrap()).unwrap();
        assert_eq!(report["differences"], json!(["one.png", "two.png"]));
        assert_eq!(fs::read(r.join("accepted/manifest.json")).unwrap(), before);
        assert_eq!(fs::read(r.join("accepted/one.png")).unwrap(), png(true));
        media::screenshots(r, "update", None).unwrap();
        let report: Value = serde_json::from_slice(&fs::read(r.join(output)).unwrap()).unwrap();
        assert_eq!(report["mode"], "update");
        assert_eq!(report["differences"], json!([]));
    }
}

#[test]
fn root_based_additional_manifest_allows_disjoint_screenshot_render() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    git(r, &["init", "-q"]);
    git(
        r,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    );
    fs::write(r.join("input"), "input").unwrap();
    fs::create_dir_all(r.join("docs")).unwrap();
    fs::write(r.join("docs/example.png"), png(true)).unwrap();
    fs::create_dir_all(r.join("accepted")).unwrap();
    fs::write(r.join("accepted/one.png"), png(true)).unwrap();
    write(
        r,
        "images.json",
        &json!({"captures":[{"path":"docs/example.png","image_sha256":hash(&png(true))}]}),
    );
    write(
        r,
        "accepted/manifest.json",
        &json!({"scenarios":[{"file":"one.png","preview":"One","alt":"one","capture_kind":"test","sha256":hash(&png(true))}]}),
    );
    let command = if cfg!(windows) {
        json!(["cmd", "/C", "exit 0"])
    } else {
        json!(["true"])
    };
    write(
        r,
        ".project/docs/config.yaml",
        &json!({"schema":1,"repository":"x/y","title":"Y","tooling_ref":"v1.0.0","media":{"manifests":[{"path":"images.json","base":".","collection":"captures","file_key":"path","hash_key":"image_sha256"}]},"screenshots":{"manifest":"accepted/manifest.json","base":"accepted","candidates":"build/captures","input_paths":["input"],"command":command}}),
    );
    let before = fs::read(r.join("images.json")).unwrap();
    media::screenshots(r, "render", None).unwrap();
    assert!(r.join("build/captures/capture.json").is_file());
    assert_eq!(fs::read(r.join("images.json")).unwrap(), before);
    assert_eq!(fs::read(r.join("docs/example.png")).unwrap(), png(true));
    let mut config: Value =
        serde_json::from_slice(&fs::read(r.join(".project/docs/config.yaml")).unwrap()).unwrap();
    config["screenshots"]["candidates"] = json!("docs");
    write(r, ".project/docs/config.yaml", &config);
    assert!(media::screenshots(r, "render", None)
        .unwrap_err()
        .to_string()
        .contains("overlaps"));
    assert_eq!(fs::read(r.join("docs/example.png")).unwrap(), png(true));
}
