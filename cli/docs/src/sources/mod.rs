//! Configured Kotlin source excerpts and reviewed whole-source inventory.
use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    roots: Vec<String>,
    manifest: String,
    #[serde(default)]
    groups: Vec<Group>,
    #[serde(default)]
    examples: Vec<Example>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    intro: Option<String>,
    output: String,
    start: Option<String>,
    end: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Example {
    source: String,
    target: String,
    start: String,
    end: String,
    region_start: Option<String>,
    region_end: Option<String>,
    prefix: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}
fn settings(root: &Path) -> Result<Settings> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&text(&path(root, ".project/docs/config.yaml")?)?)?;
    ensure!(
        value["schema"].as_u64() == Some(1),
        "Expected docs schema 1"
    );
    let s = value
        .get("sources")
        .context("Missing sources configuration")?;
    Ok(serde_yaml_ng::from_value(s.clone())?)
}
fn path(root: &Path, value: &str) -> Result<PathBuf> {
    ensure!(
        !value.is_empty()
            && !value.contains('\\')
            && Path::new(value)
                .components()
                .all(|p| matches!(p, Component::Normal(_))),
        "Unsafe source path: {value}"
    );
    let mut p = root.to_path_buf();
    for part in Path::new(value).components() {
        p.push(part);
        if let Ok(m) = fs::symlink_metadata(&p) {
            ensure!(
                !m.file_type().is_symlink(),
                "Symlink forbidden: {}",
                p.display()
            );
        }
    }
    Ok(p)
}
fn text(p: &Path) -> Result<String> {
    Ok(fs::read_to_string(p)
        .with_context(|| format!("Read {}", p.display()))?
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}
fn walk(root: &Path, p: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(p)?.file_type().is_symlink(),
        "Symlink forbidden: {}",
        p.display()
    );
    if p.is_dir() {
        let mut entries = fs::read_dir(p)?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort();
        for p in entries {
            walk(root, &p, out)?;
        }
    } else if p.extension().is_some_and(|x| x == "kt") {
        out.insert(
            p.strip_prefix(root)?
                .to_str()
                .context("Non UTF8 source path")?
                .replace('\\', "/"),
        );
    }
    Ok(())
}
fn matches(patterns: &[String], p: &str) -> Result<bool> {
    let mut b = globset::GlobSetBuilder::new();
    for v in patterns {
        b.add(globset::Glob::new(v)?);
    }
    Ok(b.build()?.is_match(p))
}
fn region<'a>(s: &'a str, start: &str, end: &str) -> Result<(usize, usize, &'a str)> {
    ensure!(
        !start.is_empty() && !end.is_empty() && start != end,
        "Distinct nonempty markers required"
    );
    ensure!(
        s.matches(start).count() == 1 && s.matches(end).count() == 1,
        "Missing or duplicate markers: {start} / {end}"
    );
    let a = s.find(start).unwrap() + start.len();
    let b = s.find(end).unwrap();
    ensure!(a <= b, "Reversed markers");
    Ok((a, b, &s[a..b]))
}
fn destination(
    root: &Path,
    outputs: &mut BTreeMap<PathBuf, String>,
    name: &str,
    body: String,
    start: Option<&str>,
    end: Option<&str>,
) -> Result<()> {
    let p = path(root, name)?;
    let value = match (start, end) {
        (None, None) => {
            ensure!(!outputs.contains_key(&p), "Output collision: {name}");
            body
        }
        (Some(a), Some(b)) => {
            let original = if let Some(v) = outputs.get(&p) {
                v.clone()
            } else {
                text(&p)?
            };
            let (i, j, _) = region(&original, a, b)?;
            format!(
                "{}\n{}\n{}",
                &original[..i],
                body.trim_end(),
                &original[j..]
            )
        }
        _ => bail!("Both destination markers required"),
    };
    outputs.insert(p, value);
    Ok(())
}
fn hidden(node: tree_sitter::Node) -> bool {
    let mut cursor = node.walk();
    let hidden = node
        .named_children(&mut cursor)
        .filter(|n| n.kind() == "modifiers")
        .any(|modifiers| {
            let mut cursor = modifiers.walk();
            let hidden = modifiers
                .named_children(&mut cursor)
                .filter(|n| n.kind() == "visibility_modifier")
                .any(|visibility| {
                    let mut cursor = visibility.walk();
                    let hidden = visibility
                        .children(&mut cursor)
                        .any(|token| matches!(token.kind(), "private" | "internal" | "protected"));
                    hidden
                });
            hidden
        });
    hidden
}

fn excerpt(node: tree_sitter::Node, s: &str, depth: usize) -> String {
    if hidden(node) {
        return String::new();
    };
    let kind = node.kind();
    let indent = "    ".repeat(depth);
    if matches!(
        kind,
        "class_declaration" | "object_declaration" | "companion_object"
    ) {
        let mut c = node.walk();
        let body = node
            .named_children(&mut c)
            .find(|n| matches!(n.kind(), "class_body" | "enum_class_body"));
        if let Some(body) = body {
            let header = s[node.start_byte()..body.start_byte()].trim();
            let mut result = format!("{indent}{header} {{\n");
            let mut c = body.walk();
            let children = body.named_children(&mut c).collect::<Vec<_>>();
            for (i, child) in children.iter().enumerate() {
                let declaration = excerpt(*child, s, depth + 1);
                if child.kind() == "enum_entry" && !declaration.is_empty() {
                    let later = children[i + 1..].iter().any(|n| n.kind() != "enum_entry");
                    let another = children[i + 1..].iter().any(|n| n.kind() == "enum_entry");
                    result.push_str(declaration.trim_end());
                    result.push_str(if !another && later { ";\n" } else { ",\n" });
                } else {
                    result.push_str(&declaration);
                }
            }
            result.push_str(&format!("{indent}}}\n"));
            return result;
        }
    }
    if matches!(
        kind,
        "function_declaration"
            | "property_declaration"
            | "secondary_constructor"
            | "anonymous_initializer"
    ) {
        if kind == "anonymous_initializer" {
            return String::new();
        };
        let mut c = node.walk();
        let inferred = kind == "property_declaration"
            && node
                .named_children(&mut c)
                .find(|n| n.kind() == "variable_declaration")
                .is_some_and(|n| n.named_child_count() == 1);
        let mut c = node.walk();
        let cutoff = node
            .children(&mut c)
            .find(|n| {
                matches!(n.kind(), "function_body" | "getter" | "setter")
                    || (!inferred && matches!(n.kind(), "property_delegate" | "="))
            })
            .map(|n| n.start_byte())
            .unwrap_or(node.end_byte());
        let mut declaration = format!(
            "{indent}{}\n",
            s[node.start_byte()..cutoff]
                .trim()
                .trim_end_matches('=')
                .trim()
        );
        if kind == "property_declaration" {
            let mut c = node.walk();
            for accessor in node
                .named_children(&mut c)
                .filter(|n| matches!(n.kind(), "getter" | "setter"))
            {
                let mut c = accessor.walk();
                let end = accessor
                    .children(&mut c)
                    .find(|n| matches!(n.kind(), "function_body" | "block" | "="))
                    .map(|n| n.start_byte())
                    .unwrap_or(accessor.end_byte());
                declaration.push_str(&format!(
                    "{indent}    {}\n",
                    s[accessor.start_byte()..end].trim()
                ));
            }
        }
        return declaration;
    }
    if kind == "enum_entry" {
        let mut c = node.walk();
        let end = node
            .named_children(&mut c)
            .find(|n| n.kind() == "class_body")
            .map(|n| n.start_byte())
            .unwrap_or(node.end_byte());
        return format!("{indent}{}\n", s[node.start_byte()..end].trim());
    }
    if matches!(
        kind,
        "type_alias" | "class_declaration" | "object_declaration"
    ) {
        return format!(
            "{indent}{}\n",
            node.utf8_text(s.as_bytes()).unwrap_or("").trim()
        );
    }
    String::new()
}
// Recognize literals/comments only to keep compatibility whitespace edits out of text.
fn literal_mask(source: &str) -> Vec<bool> {
    let b = source.as_bytes();
    let mut mask = vec![false; b.len()];
    let mut i = 0;
    while i < b.len() {
        let start = i;
        if b[i..].starts_with(b"//") {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                if b[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2
                } else {
                    i += 1
                }
            }
        } else if b[i..].starts_with(b"\"\"\"") {
            i += 3;
            while i < b.len() && !b[i..].starts_with(b"\"\"\"") {
                i += 1
            }
            i = (i + 3).min(b.len());
        } else if b[i] == b'"' || b[i] == b'\'' {
            let quote = b[i];
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i = (i + 2).min(b.len());
                } else if b[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1
                }
            }
        } else {
            i += 1;
            continue;
        }
        mask[start..i].fill(true);
    }
    mask
}
pub fn kotlin_excerpt(source: &str, name: &str) -> Result<String> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_kotlin_ng::LANGUAGE.into())?;
    let literal_protected = literal_mask(source);
    let mut bytes = source.as_bytes().to_vec();
    for m in regex::Regex::new(r"\n([ \t]*)(catch\b|\?\.)")?.find_iter(source) {
        if !literal_protected[m.start()] {
            bytes[m.start()] = b' ';
        }
    }
    let normalized = String::from_utf8(bytes.clone())?;
    let tree = parser
        .parse(&normalized, None)
        .context("Kotlin parse failed")?;
    // The grammar mistakes the soft keyword `open` in local assignment expressions.
    // Alias only files declaring that identifier; output and hashes use original text.
    let bound_open = regex::Regex::new(r"\b(?:val|var)[ \t]+open\b")?.is_match(source);
    if bound_open {
        let words = regex::Regex::new(r"\bopen\b")?;
        for m in words.find_iter(&normalized) {
            let mut n = tree
                .root_node()
                .descendant_for_byte_range(m.start(), m.end());
            let mut protected = false;
            while let Some(v) = n {
                if matches!(
                    v.kind(),
                    "string_literal" | "multiline_comment" | "line_comment" | "modifiers"
                ) {
                    protected = true;
                    break;
                }
                n = v.parent();
            }
            if !protected && !literal_protected[m.start()] {
                bytes[m.start()] = b'_';
            }
        }
    }
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.kind() == "class_body" && n.end_byte() >= 2 && bytes[n.end_byte() - 2] == b' ' {
            bytes[n.end_byte() - 2] = b';';
        }
        let mut c = n.walk();
        stack.extend(n.named_children(&mut c));
    }
    let normalized = String::from_utf8(bytes)?;
    let tree = parser
        .parse(&normalized, None)
        .context("Kotlin parse failed")?;
    if tree.root_node().has_error()
        && tree
            .root_node()
            .to_sexp()
            .replace("(MISSING _class_member_semi)", "")
            .contains("MISSING")
        || tree.root_node().to_sexp().contains("(ERROR")
    {
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if n.is_error() || n.is_missing() {
                bail!(
                    "Kotlin syntax unsupported at {name}:{} ({})",
                    source[..n.start_byte()]
                        .bytes()
                        .filter(|b| *b == b'\n')
                        .count()
                        + 1,
                    n.kind()
                );
            }
            let mut c = n.walk();
            stack.extend(n.children(&mut c));
        }
        bail!("Kotlin syntax unsupported: {name}");
    }
    let mut result = String::new();
    let mut annotations = String::new();
    let mut context = String::new();
    let mut c = tree.root_node().walk();
    for n in tree.root_node().named_children(&mut c) {
        if matches!(n.kind(), "package_header" | "import") {
            context.push_str(&source[n.start_byte()..n.end_byte()]);
            context.push('\n');
            continue;
        }
        if n.kind() == "annotated_expression"
            && source[n.start_byte()..n.end_byte()].starts_with('@')
        {
            annotations.push_str(&source[n.start_byte()..n.end_byte()]);
            annotations.push('\n');
            continue;
        }
        let declaration = excerpt(n, source, 0);
        if !declaration.is_empty() {
            result.push_str(&annotations);
            result.push_str(&declaration);
        }
        annotations.clear();
    }
    if !result.is_empty() && !context.is_empty() {
        result = format!("{context}\n{result}");
    }
    Ok(result)
}
fn source_link(output: &str, source: &str) -> String {
    let from = Path::new(output)
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .collect::<Vec<_>>();
    let to = Path::new(source).components().collect::<Vec<_>>();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut p = PathBuf::new();
    for _ in common..from.len() {
        p.push("..");
    }
    for c in &to[common..] {
        p.push(c);
    }
    p.to_string_lossy().replace('\\', "/")
}
fn render(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    let cfg = settings(root)?;
    ensure!(!cfg.roots.is_empty(), "sources.roots must not be empty");
    let mut claims: BTreeMap<PathBuf, Vec<Option<(usize, usize)>>> = BTreeMap::new();
    let mut claim = |name: &str, start: Option<&str>, end: Option<&str>| -> Result<()> {
        let p = path(root, name)?;
        ensure!(!p.is_dir(), "Output is a directory: {name}");
        for parent in p.ancestors().skip(1).take_while(|p| *p != root) {
            ensure!(
                !parent.exists() || parent.is_dir(),
                "Output parent is not a directory: {}",
                parent.display()
            );
        }
        for planned in claims.keys() {
            ensure!(
                p == *planned || (!p.starts_with(planned) && !planned.starts_with(&p)),
                "Ancestor/descendant output collision: {} and {}",
                planned.display(),
                p.display()
            );
        }
        let range = match (start, end) {
            (None, None) => None,
            (Some(a), Some(b)) => {
                let original = text(&p)?;
                let (i, j, _) = region(&original, a, b)?;
                Some((i - a.len(), j + b.len()))
            }
            _ => bail!("Both destination markers required"),
        };
        for old in claims.entry(p.clone()).or_default().iter() {
            ensure!(
                matches!((old,range),(Some((a,b)),Some((c,d))) if *b<=c || d<=*a),
                "Output collision: {}",
                p.display()
            );
        }
        claims.entry(p).or_default().push(range);
        Ok(())
    };
    claim(&cfg.manifest, None, None)?;
    for g in &cfg.groups {
        claim(&g.output, g.start.as_deref(), g.end.as_deref())?;
    }
    for e in &cfg.examples {
        claim(&e.target, Some(&e.start), Some(&e.end))?;
    }
    let mut files = BTreeSet::new();
    for r in &cfg.roots {
        walk(root, &path(root, r)?, &mut files)?;
    }
    for input in files
        .iter()
        .chain(cfg.groups.iter().filter_map(|g| g.intro.as_ref()))
        .chain(cfg.examples.iter().map(|e| &e.source))
    {
        ensure!(
            !claims.contains_key(&path(root, input)?),
            "Output collides with input: {input}"
        );
    }
    ensure!(
        !claims.contains_key(&path(root, ".project/docs/config.yaml")?),
        "Output collides with config"
    );
    let mut bodies = vec![String::new(); cfg.groups.len()];
    let mut manifest = Vec::new();
    for f in files {
        let content = text(&path(root, &f)?)?;
        let api = kotlin_excerpt(&content, &f)?;
        let mut guide = None;
        for (i, g) in cfg.groups.iter().enumerate() {
            if !api.is_empty() && matches(&g.include, &f)? && !matches(&g.exclude, &f)? {
                let link = source_link(&g.output, &f);
                bodies[i].push_str(&format!(
                    "\n## `{f}`\n\n[Source]({link})\n\n```kotlin\n{api}```\n"
                ));
                guide = Some(g.output.clone());
                break;
            }
        }
        manifest.push(serde_json::json!({"path":f,"sha256":format!("{:x}",Sha256::digest(content.as_bytes())),"guide":guide,"visibility":if api.is_empty(){"nonpublic"}else{"public"}}));
    }
    let mut outputs = BTreeMap::new();
    destination(
        root,
        &mut outputs,
        &cfg.manifest,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
        None,
        None,
    )?;
    for (i, g) in cfg.groups.iter().enumerate() {
        let intro = match &g.intro {
            Some(p) => text(&path(root, p)?)?,
            None => String::new(),
        };
        destination(
            root,
            &mut outputs,
            &g.output,
            format!(
                "{}{}",
                intro.trim_end(),
                if intro.trim_end().is_empty() {
                    bodies[i].clone()
                } else {
                    format!("\n{}", bodies[i])
                }
            ),
            g.start.as_deref(),
            g.end.as_deref(),
        )?;
    }
    for e in cfg.examples {
        let source = text(&path(root, &e.source)?)?;
        if e.source.ends_with(".kt") {
            kotlin_excerpt(&source, &e.source)?;
        }
        if let Some(mode) = &e.mode {
            ensure!(mode == "contains", "Unknown example mode: {mode}");
            ensure!(
                e.region_start.is_none() && e.region_end.is_none() && e.prefix.is_none(),
                "contains does not accept region or prefix"
            );
            let target = text(&path(root, &e.target)?)?;
            let snippet = region(&target, &e.start, &e.end)?.2;
            let fenced = snippet
                .trim()
                .strip_prefix("```kotlin\n")
                .and_then(|s| s.strip_suffix("```"))
                .context("contains target must be one Kotlin fence")?;
            let compact = |s: &str| s.split_whitespace().collect::<String>();
            ensure!(
                compact(&source).contains(&compact(fenced)) && !compact(fenced).is_empty(),
                "Source example is not contained in {}",
                e.source
            );
            continue;
        }
        let snippet = match (e.region_start.as_deref(), e.region_end.as_deref()) {
            (None, None) => source.as_str(),
            (Some(a), Some(b)) => region(&source, a, b)?.2,
            _ => bail!("Both source region markers required"),
        };
        let prefix = e.prefix.as_deref().unwrap_or("").trim_end();
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}\n\n")
        };
        let body = format!("```kotlin\n{prefix}{}\n```", snippet.trim_matches('\n'));
        destination(
            root,
            &mut outputs,
            &e.target,
            body.trim_start_matches('\n').into(),
            Some(&e.start),
            Some(&e.end),
        )?;
    }
    Ok(outputs)
}
pub fn check(root: &Path) -> Result<()> {
    for (p, expected) in render(root)? {
        ensure!(
            text(&p)? == expected,
            "Source documentation drift: {}; run sources update",
            p.display()
        );
    }
    Ok(())
}
pub fn update(root: &Path) -> Result<()> {
    let outputs = render(root)?;
    for (p, s) in outputs {
        fs::create_dir_all(p.parent().unwrap())?;
        fs::write(p, s)?;
    }
    Ok(())
}
pub fn configured(root: &Path) -> Result<bool> {
    let p = path(root, ".project/docs/config.yaml")?;
    if !p.exists() {
        return Ok(false);
    }
    let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text(&p)?)?;
    Ok(v.get("sources").is_some())
}
pub fn init(root: &Path) -> Result<()> {
    init_with_roots(root, &[])
}
/// Add an inventory configuration without replacing existing reviewed settings.
pub fn init_with_roots(root: &Path, explicit: &[String]) -> Result<()> {
    if configured(root)? {
        settings(root)?;
        return Ok(());
    }
    let config_path = path(root, ".project/docs/config.yaml")?;
    let mut config: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text(&config_path)?)?;
    let mut roots = if explicit.is_empty() {
        inferred_roots(root)?
    } else {
        explicit.to_vec()
    };
    ensure!(
        !roots.is_empty(),
        "No selected library source roots; pass --source-root for custom layouts"
    );
    roots.sort();
    roots.dedup();
    for r in &roots {
        ensure!(path(root, r)?.is_dir(), "Missing source root: {r}");
    }
    config["sources"] = serde_yaml_ng::to_value(
        serde_json::json!({"roots":roots,"manifest":".project/docs/sources.json","groups":[],"examples":[]}),
    )?;
    fs::write(config_path, serde_yaml_ng::to_string(&config)?)?;
    Ok(())
}

pub(crate) fn inferred_roots(root: &Path) -> Result<Vec<String>> {
    let mut roots = Vec::new();
    {
        let setup = path(root, ".project/setup/config.yaml")?;
        if setup.is_file() {
            let setup: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text(&setup)?)?;
            if let Some(modules) = setup["android_modules"].as_mapping() {
                for (_, module) in modules {
                    if module["kind"].as_str() == Some("library")
                        && module["selected"].as_bool() == Some(true)
                    {
                        if let Some(dir) = module["directory"].as_str() {
                            for source in ["src/main/java", "src/main/kotlin"] {
                                let candidate = format!("{dir}/{source}");
                                if path(root, &candidate)?.is_dir() {
                                    roots.push(candidate);
                                }
                            }
                        }
                    }
                }
            }
        }
        for source in ["src/main/java", "src/main/kotlin"] {
            if path(root, source)?.is_dir() {
                roots.push(source.into());
            }
        }
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}
