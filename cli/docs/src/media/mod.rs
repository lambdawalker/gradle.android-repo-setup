pub mod inventory;
// Explicitly reviewed documentation media and reproducible screenshot candidates.
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    #[serde(default)]
    manifest: String,
    #[serde(default)]
    base: String,
    #[serde(default)]
    required_ids: Vec<String>,
    source: Option<Source>,
    #[serde(default)]
    frames: Vec<Frame>,
    #[serde(default)]
    manifests: Vec<Accepted>,
    #[serde(default)]
    candidates: String,
    #[serde(default)]
    input_paths: Vec<String>,
    #[serde(default)]
    command: Vec<String>,
    legacy_base: Option<String>,
    provenance: Option<String>,
    evidence: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    path: String,
    #[serde(default)]
    min_bytes: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    id: String,
    candidate: String,
    evidence: String,
    command: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Accepted {
    path: String,
    base: String,
    collection: String,
    #[serde(default = "file_key")]
    file_key: String,
    #[serde(default = "hash_key")]
    hash_key: String,
    #[serde(default)]
    required_files: Vec<String>,
    #[serde(default)]
    require_source: bool,
    #[serde(default)]
    require_dimensions: bool,
}
fn hash_key() -> String {
    "sha256".into()
}
fn file_key() -> String {
    "file".into()
}
pub fn path(root: &Path, value: &str) -> Result<PathBuf> {
    ensure!(
        !value.is_empty() && !value.contains(['\\', ':']),
        "Invalid media path: {value}"
    );
    if value == "." {
        return Ok(root.to_path_buf());
    }
    let mut p = root.to_path_buf();
    for c in Path::new(value).components() {
        ensure!(
            matches!(c, Component::Normal(_)),
            "Invalid media path: {value}"
        );
        p.push(c);
        if let Ok(m) = fs::symlink_metadata(&p) {
            ensure!(!m.file_type().is_symlink(), "Symlink media path: {value}");
        }
    }
    Ok(p)
}
fn load(p: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(
        &fs::read(p).with_context(|| format!("Read {}", p.display()))?,
    )?)
}
fn write(p: &Path, v: &Value) -> Result<()> {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d)?;
    }
    fs::write(p, format!("{}\n", serde_json::to_string_pretty(v)?))?;
    Ok(())
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hash(p: &Path) -> Result<String> {
    Ok(sha(&fs::read(p)?))
}
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().with_context(|| format!("Missing media {k}"))
}
fn full_sha(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        s.len() == 40
            && s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}
pub fn pixels(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = d.read_info()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let i = r.next_frame(&mut buf)?;
    let b = &buf[..i.buffer_size()];
    let mut out = Vec::new();
    match i.color_type {
        png::ColorType::Rgba => out.extend_from_slice(b),
        png::ColorType::Rgb => {
            for c in b.chunks_exact(3) {
                out.extend_from_slice(&[c[0], c[1], c[2], 255])
            }
        }
        png::ColorType::Grayscale => {
            for x in b {
                out.extend_from_slice(&[*x, *x, *x, 255])
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for c in b.chunks_exact(2) {
                out.extend_from_slice(&[c[0], c[0], c[0], c[1]])
            }
        }
        _ => anyhow::bail!("Unsupported PNG"),
    };
    Ok((i.width, i.height, out))
}
fn run(root: &Path, args: &[String]) -> Result<()> {
    ensure!(!args.is_empty(), "Missing render command");
    let executable = if cfg!(windows) && args[0] == "./gradlew" {
        "gradlew.bat"
    } else {
        &args[0]
    };
    ensure!(
        Command::new(executable)
            .args(&args[1..])
            .current_dir(root)
            .status()?
            .success(),
        "Media command failed"
    );
    Ok(())
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let o = Command::new("git").args(args).current_dir(root).output()?;
    ensure!(o.status.success(), "Git media input inventory failed");
    Ok(o.stdout)
}
pub fn input_hash(root: &Path, paths: &[String]) -> Result<String> {
    ensure!(!paths.is_empty(), "Missing screenshot input_paths");
    for p in paths {
        path(root, p)?;
    }
    let mut args = vec![
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    let names = git(root, &args)?;
    let names: BTreeSet<_> = names.split(|b| *b == 0).filter(|s| !s.is_empty()).collect();
    let mut h = Sha256::new();
    for name in names {
        let name = std::str::from_utf8(name)?;
        h.update(name.as_bytes());
        h.update([0]);
        let p = path(root, name)?;
        match fs::read(p) {
            Ok(b) => h.update(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => h.update(b"<deleted>"),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(format!("{:x}", h.finalize()))
}
fn walk(p: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in fs::read_dir(p)? {
        let e = e?;
        ensure!(!e.file_type()?.is_symlink(), "Symlink capture");
        if e.file_type()?.is_dir() {
            walk(&e.path(), out)?
        } else if e.path().extension().is_some_and(|s| s == "png") {
            out.push(e.path())
        }
    }
    out.sort();
    Ok(())
}
fn scenarios(v: &Value) -> Result<&Vec<Value>> {
    let s = v["scenarios"].as_array().context("Missing scenarios")?;
    ensure!(!s.is_empty(), "Empty scenarios");
    let mut files = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut previews = BTreeSet::new();
    for item in s {
        ensure!(files.insert(text(item, "file")?), "Duplicate scenario file");
        if let Some(id) = item["id"].as_str() {
            ensure!(ids.insert(id), "Duplicate scenario id");
        }
        if let Some(p) = item["preview"].as_str() {
            ensure!(previews.insert(p), "Duplicate preview");
        }
    }
    Ok(s)
}
fn validate(root: &Path, s: &Spec, v: &Value, legacy: bool) -> Result<()> {
    let base = path(root, &s.base)?;
    for item in scenarios(v)? {
        let p = path(&base, text(item, "file")?)?;
        let bytes = fs::read(&p)?;
        ensure!(
            sha(&bytes) == text(item, "sha256")?,
            "Unrecorded accepted image change: {}",
            p.display()
        );
        pixels(&bytes)?;
        for key in ["fixture", "guide", "demo"] {
            if let Some(p) = item[key].as_str() {
                ensure!(path(root, p)?.exists(), "Missing manifest target {p}");
            }
        }
        if legacy {
            if let Some(b) = &s.legacy_base {
                ensure!(
                    fs::read(path(&path(root, b)?, text(item, "file")?)?)? == bytes,
                    "Legacy screenshot differs"
                );
            }
        }
    }
    for id in &s.required_ids {
        ensure!(
            scenarios(v)?.iter().any(|i| i["id"] == *id),
            "Required media scenario missing: {id}"
        );
    }
    Ok(())
}
fn accepted(root: &Path, s: &Spec) -> Result<()> {
    for a in &s.manifests {
        let v = load(&path(root, &a.path)?)?;
        if a.require_source {
            ensure!(full_sha(&v["source"]), "Invalid accepted source");
        }
        let entries = v[&a.collection]
            .as_array()
            .context("Invalid accepted collection")?;
        let mut names = BTreeSet::new();
        for i in entries {
            let name = text(i, &a.file_key)?;
            ensure!(names.insert(name.to_owned()), "Duplicate accepted image");
            let b = fs::read(path(&path(root, &a.base)?, name)?)?;
            ensure!(
                sha(&b) == text(i, &a.hash_key)?,
                "Accepted image hash mismatch"
            );
            let (w, h, _) = pixels(&b)?;
            if a.require_dimensions {
                ensure!(
                    i["width"] == w && i["height"] == h,
                    "Accepted image dimensions mismatch"
                );
            }
        }
        if !a.required_files.is_empty() {
            let expected: BTreeSet<_> = a.required_files.iter().cloned().collect();
            ensure!(
                expected.len() == a.required_files.len(),
                "Duplicate required accepted image"
            );
            ensure!(names == expected, "Missing or unexpected accepted image");
        }
    }
    Ok(())
}
fn source(root: &Path, s: &Spec, v: &Value) -> Result<Option<String>> {
    if let Some(src) = &s.source {
        let p = path(root, &src.path)?;
        ensure!(
            fs::metadata(&p)?.len() >= src.min_bytes,
            "Hydrate media source"
        );
        let h = hash(&p)?;
        ensure!(
            v["recording"]["sha256"] == h,
            "Recording provenance changed"
        );
        Ok(Some(h))
    } else {
        Ok(None)
    }
}
pub fn verify(root: &Path) -> Result<()> {
    let c = crate::config::Config::load(root)?;
    for (kind, value) in [("media", c.media), ("screenshots", c.screenshots)] {
        if let Some(value) = value {
            let s: Spec = serde_json::from_value(value)?;
            accepted(root, &s)?;
            if !s.manifest.is_empty() {
                let v = load(&path(root, &s.manifest)?)?;
                validate(root, &s, &v, true)?;
                source(root, &s, &v)?;
                if kind == "screenshots" {
                    for i in scenarios(&v)? {
                        for k in ["alt", "preview", "capture_kind"] {
                            ensure!(!text(i, k)?.is_empty(), "Incomplete screenshot scenario");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

// Resolve every protected destination before any renderer or destructive cleanup.
fn protected(root: &Path, directories: bool) -> Result<Vec<PathBuf>> {
    let c = crate::config::Config::load(root)?;
    let mut paths = vec![
        path(root, ".project/docs/config.yaml")?,
        path(root, ".project/setup/config.yaml")?,
    ];
    for raw in [c.media, c.screenshots].into_iter().flatten() {
        let s: Spec = serde_json::from_value(raw)?;
        if let Some(source) = &s.source {
            paths.push(path(root, &source.path)?);
        }
        for input in &s.input_paths {
            paths.push(path(root, input)?);
        }
        if !s.manifest.is_empty() {
            paths.push(path(root, &s.manifest)?);
            let v = load(&path(root, &s.manifest)?)?;
            let base = path(root, &s.base)?;
            if directories && base != root {
                paths.push(base.clone());
            }
            let legacy = s
                .legacy_base
                .as_deref()
                .map(|p| path(root, p))
                .transpose()?;
            if directories {
                if let Some(p) = &legacy {
                    if p != root {
                        paths.push(p.clone());
                    }
                }
            }
            for item in scenarios(&v)? {
                let file = text(item, "file")?;
                paths.push(path(&base, file)?);
                if let Some(base) = &legacy {
                    paths.push(path(base, file)?);
                }
                for key in ["fixture", "guide", "demo"] {
                    if let Some(p) = item[key].as_str() {
                        paths.push(path(root, p)?);
                    }
                }
            }
        }
        for a in &s.manifests {
            paths.push(path(root, &a.path)?);
            let base = path(root, &a.base)?;
            if directories && base != root {
                paths.push(base.clone());
            }
            let v = load(&path(root, &a.path)?)?;
            for item in v[&a.collection]
                .as_array()
                .context("Invalid accepted collection")?
            {
                paths.push(path(&base, text(item, &a.file_key)?)?);
            }
        }
    }
    Ok(paths)
}

pub fn media(root: &Path, action: &str) -> Result<()> {
    let c = crate::config::Config::load(root)?;
    let raw = c.media.context("No media configuration")?;
    let config_hash = sha(&serde_json::to_vec(&raw)?);
    let s: Spec = serde_json::from_value(raw)?;
    accepted(root, &s)?;
    if s.manifest.is_empty() {
        ensure!(action == "verify", "No primary frame manifest configured");
        return Ok(());
    }
    let mp = path(root, &s.manifest)?;
    let mut v = load(&mp)?;
    validate(root, &s, &v, true)?;
    let source_hash = source(root, &s, &v)?;
    if action == "verify" {
        return Ok(());
    }
    ensure!(
        ["render", "compare", "accept"].contains(&action),
        "Invalid media action"
    );
    let protected = protected(root, false)?;
    let mut outputs: Vec<PathBuf> = Vec::new();
    let mut ids = BTreeSet::new();
    let mut updates = Vec::new();
    for f in &s.frames {
        ensure!(ids.insert(&f.id), "Duplicate frame");
        let index = scenarios(&v)?
            .iter()
            .position(|i| i["id"] == f.id)
            .context("Frame scenario missing")?;
        let candidate = path(root, &f.candidate)?;
        let evidence = path(root, &f.evidence)?;
        for output in [&candidate, &evidence] {
            ensure!(
                !protected.iter().any(|p| overlaps(output, p)),
                "Frame output overlaps protected media: {}",
                output.display()
            );
            ensure!(
                !outputs.iter().any(|p| overlaps(output, p)),
                "Frame output collision: {}",
                output.display()
            );
            ensure!(!output.is_dir(), "Frame output is a directory");
            outputs.push(output.clone());
        }
        ensure!(!f.command.is_empty(), "Missing render command");
        for arg in &f.command {
            ensure!(!arg.contains('\0'), "Invalid command argument");
        }
        updates.push((f, index, candidate, evidence));
    }
    ensure!(!updates.is_empty(), "No configured frames");
    if action != "accept" {
        for (_, _, p, e) in &updates {
            for output in [p, e] {
                match fs::remove_file(output) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        for (f, _, p, e) in &updates {
            if let Some(d) = p.parent() {
                fs::create_dir_all(d)?;
            }
            run(root, &f.command)?;
            pixels(&fs::read(p)?)?;
            write(
                e,
                &json!({"source_sha256":source_hash,"output_sha256":hash(p)?,"tool":f.command[0],"command":f.command,"config_sha256":config_hash}),
            )?;
        }
    }
    let mut copies = Vec::new();
    for (_, index, p, e) in updates {
        let evidence = load(&e)?;
        let bytes = fs::read(p)?;
        pixels(&bytes)?;
        ensure!(
            evidence["config_sha256"] == config_hash,
            "Frame render configuration changed"
        );
        ensure!(
            evidence["source_sha256"]
                == source_hash
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null)
                && evidence["output_sha256"] == sha(&bytes),
            "Stale frame candidate"
        );
        let target = path(&path(root, &s.base)?, text(&v["scenarios"][index], "file")?)?;
        if action == "compare" {
            ensure!(
                fs::read(target)? == bytes,
                "Candidate differs; review before accepting"
            );
        } else if action == "accept" {
            v["scenarios"][index]["sha256"] = json!(sha(&bytes));
            v["scenarios"][index]["render"]["command"] = evidence["command"].clone();
            v["scenarios"][index]["render"]["tool"] = evidence["tool"].clone();
            copies.push((target, bytes));
        }
    }
    for (p, b) in copies {
        fs::write(p, b)?;
    }
    if action == "accept" {
        write(&mp, &v)?;
    }
    Ok(())
}
pub fn screenshots(root: &Path, action: &str, directory: Option<&str>) -> Result<()> {
    if action == "inventory" {
        return inventory::generate(
            root,
            directory.context("screenshots inventory requires module")?,
        );
    }
    let c = crate::config::Config::load(root)?;
    let raw = c.screenshots.context("No screenshots configuration")?;
    let config_hash = sha(&serde_json::to_vec(&raw)?);
    let s: Spec = serde_json::from_value(raw)?;
    let mp = path(root, &s.manifest)?;
    let mut v = load(&mp)?;
    validate(root, &s, &v, action != "update")?;
    for i in scenarios(&v)? {
        for k in ["alt", "preview", "capture_kind"] {
            ensure!(!text(i, k)?.is_empty(), "Incomplete scenario");
        }
    }
    if action == "manifest" {
        return Ok(());
    }
    ensure!(
        ["render", "check", "update"].contains(&action),
        "Invalid screenshots action"
    );
    let dir = path(root, directory.unwrap_or(&s.candidates))?;
    let capture_path = dir.join("capture.json");
    let inputs = input_hash(root, &s.input_paths)?;
    if action == "render" {
        ensure!(directory.is_none(), "render uses configured candidates");
        ensure!(dir != root, "Candidate directory is repository root");
        for p in protected(root, true)? {
            ensure!(
                !overlaps(&dir, &p),
                "Candidate directory overlaps protected media: {}",
                p.display()
            );
        }
        for input in &s.input_paths {
            ensure!(
                !overlaps(&dir, &path(root, input)?),
                "Candidate directory overlaps source inputs"
            );
        }
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        run(root, &s.command)?;
        ensure!(
            inputs == input_hash(root, &s.input_paths)?,
            "Inputs changed while rendering"
        );
        fs::create_dir_all(&dir)?;
        let commit = String::from_utf8(git(root, &["rev-parse", "HEAD"])?)?;
        return write(
            &capture_path,
            &json!({"source_commit":commit.trim(),"config_sha256":config_hash,"command":s.command,"input_sha256":inputs,"working_tree_dirty":!git(root,&["status","--porcelain"])?.is_empty(),"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH}),
        );
    }
    let evidence = s
        .evidence
        .as_deref()
        .map(|value| path(root, value))
        .transpose()?;
    if let Some(output) = &evidence {
        for p in protected(root, false)? {
            ensure!(
                !overlaps(output, &p),
                "Screenshot evidence overlaps protected media"
            );
        }
        let c = crate::config::Config::load(root)?;
        for raw in [c.media, c.screenshots].into_iter().flatten() {
            let spec: Spec = serde_json::from_value(raw)?;
            if !spec.candidates.is_empty() {
                ensure!(
                    !overlaps(output, &path(root, &spec.candidates)?),
                    "Screenshot evidence overlaps candidates"
                );
            }
            for frame in spec.frames {
                for value in [frame.candidate, frame.evidence] {
                    ensure!(
                        !overlaps(output, &path(root, &value)?),
                        "Screenshot evidence overlaps frame output"
                    );
                }
            }
        }
        ensure!(
            !overlaps(output, &dir),
            "Screenshot evidence overlaps candidates"
        );
        ensure!(!output.is_dir(), "Screenshot evidence is a directory");
    }
    let capture = load(&capture_path)?;
    ensure!(
        capture["config_sha256"] == config_hash,
        "Capture render configuration changed"
    );
    ensure!(
        capture["input_sha256"] == inputs && full_sha(&capture["source_commit"]),
        "Stale capture provenance; render again"
    );
    let mut files = Vec::new();
    walk(&dir, &mut files)?;
    let mut copies = Vec::new();
    let mut differences = Vec::new();
    let mut used = BTreeSet::new();
    for (index, i) in scenarios(&v)?.iter().enumerate() {
        let preview = text(i, "preview")?;
        ensure!(
            crate::matches("[A-Za-z][A-Za-z0-9]*", preview),
            "Invalid preview method"
        );
        let re = regex::Regex::new(&format!(
            r"(?:^|[/\.]){}(?:[_. /]|$)",
            regex::escape(preview)
        ))?;
        let found: Vec<_> = files
            .iter()
            .filter(|p| {
                re.is_match(
                    &p.strip_prefix(&dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                )
            })
            .collect();
        ensure!(
            found.len() == 1,
            "Expected exactly one capture for {preview}, found {}",
            found.len()
        );
        ensure!(used.insert(found[0]), "Ambiguous capture");
        let bytes = fs::read(found[0])?;
        let decoded = pixels(&bytes)?;
        let target = path(&path(root, &s.base)?, text(i, "file")?)?;
        if action == "check" && pixels(&fs::read(&target)?)? != decoded {
            differences.push(text(i, "file")?.to_owned());
        }
        copies.push((index, target, text(i, "file")?.to_owned(), bytes));
    }
    let report = if evidence.is_some() {
        let source = String::from_utf8(git(root, &["rev-parse", "HEAD"])?)?;
        Some(
            json!({"source_commit":source.trim(),"input_sha256":inputs,"mode":action,"compared":scenarios(&v)?.iter().map(|i|text(i,"file").map(str::to_owned)).collect::<Result<Vec<_>>>()?,"differences":differences}),
        )
    } else {
        None
    };
    if action == "check" {
        if let (Some(path), Some(report)) = (&evidence, &report) {
            write(path, report)?;
        }
        ensure!(differences.is_empty(), "Documentation screenshot changes: {}. Review candidates; never accept automatically in CI.",differences.join(", "));
    }
    if action == "update" {
        let legacy = if let Some(b) = &s.legacy_base {
            Some(path(root, b)?)
        } else {
            None
        };
        let mut writes = Vec::new();
        for (index, p, name, b) in copies {
            if let Some(base) = &legacy {
                writes.push((path(base, &name)?, b.clone()));
            }
            v["scenarios"][index]["sha256"] = json!(sha(&b));
            v["scenarios"][index]["source_commit"] = capture["source_commit"].clone();
            v["scenarios"][index]["capture_environment"] = capture.clone();
            v["scenarios"][index]["input_sha256"] = json!(inputs);
            v["scenarios"][index]["provenance"]=json!(s.provenance.as_deref().unwrap_or("Locally rendered candidates accepted with screenshots:update; review images before committing."));
            writes.push((p, b));
        }
        ensure!(
            inputs == input_hash(root, &s.input_paths)?,
            "Inputs changed before acceptance"
        );
        for (p, b) in writes {
            if let Some(d) = p.parent() {
                fs::create_dir_all(d)?;
            }
            fs::write(p, b)?;
        }
        write(&mp, &v)?;
        if let (Some(path), Some(report)) = (&evidence, &report) {
            write(path, report)?;
        }
    }
    Ok(())
}
