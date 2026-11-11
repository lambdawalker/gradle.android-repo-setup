//! Opt-in Zola backend. Inputs are read-only; all generated content lives in a temporary site.
use super::{children, history, read, string, Config};
use anyhow::{bail, ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use unicode_normalization::UnicodeNormalization;

const ZOLA_VERSION: &str = "0.23.6";
fn write(root: &Path, path: &str, bytes: impl AsRef<[u8]>) -> Result<()> {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, bytes)?;
    Ok(())
}
fn git(root: &Path, reference: &str, path: &str) -> Result<Vec<u8>> {
    let result = Command::new("git")
        .current_dir(root)
        .args(["show", &format!("{reference}:{path}")])
        .output()?;
    ensure!(
        result.status.success(),
        "Could not read {path} at {reference}: {}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok(result.stdout)
}
fn bytes(root: &Path, reference: Option<&str>, path: &str) -> Result<Vec<u8>> {
    if let Some(r) = reference {
        git(root, r, path)
    } else {
        Ok(fs::read(root.join(path))?)
    }
}
fn text(root: &Path, reference: Option<&str>, path: &str) -> Result<String> {
    Ok(String::from_utf8(bytes(root, reference, path)?)?
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}
fn guides(root: &Path, reference: Option<&str>) -> Result<Vec<String>> {
    if let Some(r) = reference {
        let result = Command::new("git")
            .current_dir(root)
            .args(["ls-tree", "-r", "--name-only", r, "--", "docs/agents/"])
            .output()?;
        ensure!(
            result.status.success(),
            "Could not list documentation at {r}"
        );
        Ok(String::from_utf8(result.stdout)?
            .lines()
            .filter(|p| p.ends_with(".md"))
            .map(str::to_owned)
            .collect())
    } else {
        fn visit(root: &Path, path: &Path, out: &mut Vec<String>) -> Result<()> {
            for child in children(path)? {
                if child.is_dir() {
                    visit(root, &child, out)?
                } else if child.extension().is_some_and(|x| x == "md") {
                    out.push(
                        child
                            .strip_prefix(root)?
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
            Ok(())
        }
        let mut out = vec![];
        visit(root, &root.join("docs/agents"), &mut out)?;
        Ok(out)
    }
}
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for p in children(from)? {
        let dest = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy_tree(&p, &dest)?
        } else {
            fs::create_dir_all(to)?;
            fs::copy(p, dest)?;
        }
    }
    Ok(())
}
fn slug(path: &str) -> String {
    let s = path
        .strip_prefix("docs/agents/")
        .unwrap_or(path)
        .trim_end_matches(".md");
    if s == "index" {
        String::new()
    } else {
        s.to_owned()
    }
}
fn route(scope: &str, guide: &str) -> String {
    if guide.is_empty() {
        scope.to_owned()
    } else {
        format!("{scope}{guide}/")
    }
}
fn raw_route(scope: &str, guide: &str) -> String {
    format!(
        "{scope}raw/{}.md",
        if guide.is_empty() { "index" } else { guide }
    )
}
fn normalize(path: &str) -> String {
    let mut parts = vec![];
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}
fn title_body(markdown: &str) -> Result<(String, String)> {
    use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
    let mut active = None;
    for (event, range) in
        Parser::new_ext(markdown, pulldown_cmark::Options::ENABLE_HEADING_ATTRIBUTES)
            .into_offset_iter()
    {
        match event {
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            }) => active = Some((range, String::new())),
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, title)) = &mut active {
                    title.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(HeadingLevel::H1)) => {
                if let Some((range, title)) = active.take() {
                    let mut body = markdown.to_owned();
                    body.replace_range(range, "");
                    return Ok((title, body));
                }
            }
            _ => {}
        }
    }
    bail!("Documentation page is missing its H1 title")
}
fn heading_ranges(markdown: &str) -> Vec<(std::ops::Range<usize>, String)> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut headings = Vec::new();
    let mut active = None;
    for (event, range) in Parser::new(markdown).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { .. }) => active = Some((range, String::new())),
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, text)) = &mut active {
                    text.push_str(&t)
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(heading) = active.take() {
                    headings.push(heading)
                }
            }
            _ => {}
        }
    }
    headings
}
fn anchors(english: &str, translated: &str) -> String {
    let separator = Regex::new(r"[-\s]+").unwrap();
    let mut used = BTreeSet::new();
    let ids: Vec<String> = heading_ranges(english)
        .into_iter()
        .map(|(_, heading)| {
            let ascii: String = heading
                .nfkd()
                .filter(char::is_ascii)
                .filter(|c| {
                    c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || c.is_ascii_whitespace()
                })
                .collect();
            let base = separator.replace_all(ascii.trim(), "-").to_lowercase();
            let mut id = base.clone();
            let mut i = 1;
            while id.is_empty() || used.contains(&id) {
                id = format!("{base}_{i}");
                i += 1;
            }
            used.insert(id.clone());
            id
        })
        .collect();
    let mut output = translated.to_owned();
    for ((range, _), id) in heading_ranges(translated).into_iter().zip(ids).rev() {
        let original = &translated[range.clone()];
        if original.contains("{#") {
            continue;
        }
        let mut lines = original.lines();
        if let Some(first) = lines.next() {
            let replacement = format!(
                "{} {{#{id}}}{}",
                first.trim_end().trim_end_matches('#').trim_end(),
                if original.ends_with('\n') { "\n" } else { "" }
            );
            // ATX headings use one line; setext headings retain the underline.
            let replacement = if let Some(underline) = lines.next() {
                format!("{} {{#{id}}}\n{underline}\n", first.trim_end())
            } else {
                replacement
            };
            output.replace_range(range, &replacement);
        }
    }
    output
}
fn raw_anchors(markdown: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    let mut result = markdown.to_owned();
    let mut edits = Vec::new();
    for (event, range) in
        Parser::new_ext(markdown, Options::ENABLE_HEADING_ATTRIBUTES).into_offset_iter()
    {
        if let Event::Start(Tag::Heading { id: Some(id), .. }) = event {
            let original = &markdown[range.clone()];
            let attribute = format!("{{#{id}}}");
            if original.contains(&attribute) {
                edits.push((
                    range,
                    format!(
                        "<a id=\"{}\"></a>\n{}",
                        id.replace('"', "&quot;"),
                        original.replacen(&attribute, "", 1)
                    ),
                ));
            }
        }
    }
    for (range, replacement) in edits.into_iter().rev() {
        result.replace_range(range, &replacement);
    }
    result
}
fn localized(
    root: &Path,
    reference: Option<&str>,
    translation: Option<&str>,
    path: &str,
    lang: &str,
    english: &str,
) -> Result<(String, bool)> {
    if lang == "en" {
        return Ok((english.into(), false));
    }
    let key = path
        .strip_prefix("docs/agents/")
        .unwrap_or(path.strip_prefix("docs/").unwrap_or(path));
    let tr = translation.or(reference);
    let manifest = text(root, tr, "docs/es/translations.json").or_else(|error| {
        if translation.is_some() {
            text(root, tr, "translations.json")
        } else {
            Err(error)
        }
    });
    let translated = text(root, tr, &format!("docs/es/{key}")).or_else(|error| {
        if translation.is_some() {
            text(root, tr, key)
        } else {
            Err(error)
        }
    });
    if let (Ok(manifest), Ok(translated)) = (manifest, translated) {
        let manifest: Value =
            serde_json::from_str(&manifest).context("Invalid Spanish translation hash manifest")?;
        let hash = format!("{:x}", Sha256::digest(english.as_bytes()));
        if manifest[key].as_str() == Some(&hash) {
            return Ok((anchors(english, &translated), false));
        }
    }
    Ok((english.into(), true))
}
fn config_set(config: &str, section: &str, key: &str, value: &str) -> String {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut found = false;
    let mut section_seen = section.is_empty();
    for line in config.lines() {
        let trim = line.trim();
        if trim.starts_with('[') && trim.ends_with(']') {
            if current == section && !found {
                out.push(format!("{key} = {value}"));
                found = true;
            }
            current = trim[1..trim.len() - 1].to_owned();
            if current == section {
                section_seen = true;
            }
        }
        if current == section && trim.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
            out.push(format!("{key} = {value}"));
            found = true;
        } else {
            out.push(line.to_owned());
        }
    }
    if !found {
        if !section_seen {
            out.push(format!("[{section}]"));
        }
        out.push(format!("{key} = {value}"));
    }
    out.join("\n") + "\n"
}
fn base_url(config: &str, base: Option<&str>) -> Result<(String, String)> {
    let regex = Regex::new(r#"(?m)^\s*base_url\s*=\s*["']([^"']+)["']"#)?;
    let configured = regex
        .captures(config)
        .context("sites/zola.toml needs an absolute base_url")?[1]
        .to_owned();
    let (scheme, rest) = configured
        .split_once("://")
        .context("Zola base_url must include https:// or http://")?;
    let (host, _path) = rest.split_once('/').unwrap_or((rest, ""));
    ensure!(!host.is_empty(), "Zola base_url must include a host");
    let url = if let Some(base) = base {
        if base.contains("://") {
            base.to_owned()
        } else {
            format!("{scheme}://{host}/{}", base.trim_matches('/'))
        }
    } else {
        configured
    };
    let rest = url.split_once("://").unwrap().1;
    let prefix = format!(
        "/{}/",
        rest.split_once('/')
            .map(|(_, p)| p)
            .unwrap_or("")
            .trim_matches('/')
    )
    .replace("//", "/");
    Ok((format!("{}/", url.trim_end_matches('/')), prefix))
}
struct Rewriter<'a> {
    root: &'a Path,
    stage: &'a Path,
    reference: Option<&'a str>,
    scope: &'a str,
    prefix: &'a str,
    source_path: &'a str,
    source_ref: &'a str,
    pages: &'a BTreeMap<String, String>,
    repository: &'a str,
}
// Locate the outer label, respecting nested image brackets and escaped punctuation.
fn closing_label(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'`' => {
                let start = i;
                while i < bytes.len() && bytes[i] == b'`' {
                    i += 1;
                }
                let fence = &source[start..i];
                if let Some(end) = source[i..].find(fence) {
                    i += end + fence.len();
                }
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}
fn inline_destination(source: &str, label_end: usize) -> Option<std::ops::Range<usize>> {
    let bytes = source.as_bytes();
    let mut start = label_end + 1;
    if bytes.get(start) != Some(&b'(') {
        return None;
    }
    start += 1;
    while bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
        start += 1;
    }
    if bytes.get(start) == Some(&b'<') {
        start += 1;
        let mut end = start;
        while end < bytes.len() {
            if bytes[end] == b'\\' {
                end += 2;
            } else if bytes[end] == b'>' {
                return Some(start..end);
            } else {
                end += 1;
            }
        }
    } else {
        let mut end = start;
        let mut parentheses = 0;
        while end < bytes.len() {
            match bytes[end] {
                b'\\' => {
                    end += 2;
                    continue;
                }
                b'(' => parentheses += 1,
                b')' if parentheses == 0 => return Some(start..end),
                b')' => parentheses -= 1,
                byte if byte.is_ascii_whitespace() => return Some(start..end),
                _ => {}
            }
            end += 1;
        }
    }
    None
}
impl Rewriter<'_> {
    fn rewrite(&self, markdown: &str, raw: bool) -> Result<String> {
        use pulldown_cmark::{Event, LinkType, Parser, Tag};
        let mut edits = Vec::new();
        for (event, range) in Parser::new(markdown).into_offset_iter() {
            let (target, image, link_type) = match event {
                Event::Start(Tag::Link {
                    dest_url,
                    link_type,
                    ..
                }) => (dest_url.to_string(), false, link_type),
                Event::Start(Tag::Image {
                    dest_url,
                    link_type,
                    ..
                }) => (dest_url.to_string(), true, link_type),
                _ => continue,
            };
            if target.starts_with('#') || target.starts_with('/') || target.contains(':') {
                continue;
            }
            let (path, fragment) = target
                .split_once('#')
                .map(|(p, f)| (p, format!("#{f}")))
                .unwrap_or((&target, String::new()));
            let parent = self
                .source_path
                .rsplit_once('/')
                .map(|(p, _)| p)
                .unwrap_or("");
            let resolved = normalize(&format!("{parent}/{path}"));
            let resolved = if path.ends_with("IMPORT.md") {
                "IMPORT.md".to_owned()
            } else {
                resolved
            };
            let href = if let Some(guide) = self.pages.get(&resolved) {
                if raw {
                    raw_route(self.scope, guide)
                } else {
                    route(self.scope, guide)
                }
            } else if image {
                let media = format!("media/{resolved}");
                write(
                    &self.stage.join("static"),
                    &format!("{}{media}", self.scope.trim_start_matches(self.prefix)),
                    bytes(self.root, self.reference, &resolved)
                        .with_context(|| format!("Missing documentation image {resolved}"))?,
                )?;
                format!("{}{media}", self.scope)
            } else {
                format!(
                    "https://github.com/{}/blob/{}/{}",
                    self.repository, self.source_ref, resolved
                )
            };
            let original = &markdown[range.clone()];
            let label_end = closing_label(original)
                .with_context(|| format!("Could not locate Markdown link label in {original}"))?;
            let destination = format!("{href}{fragment}");
            if link_type == LinkType::Inline {
                let span = inline_destination(original, label_end).with_context(|| {
                    format!("Could not locate Markdown link destination in {original}")
                })?;
                edits.push((
                    range.start + span.start..range.start + span.end,
                    destination,
                ));
            } else {
                // Replacing only the reference suffix preserves nested image labels and their own edits.
                edits.push((
                    range.start + label_end + 1..range.end,
                    format!("({destination})"),
                ));
            }
        }
        edits.sort_by_key(|(range, _)| (range.start, range.end));
        let mut result = markdown.to_owned();
        for (range, replacement) in edits.into_iter().rev() {
            result.replace_range(range, &replacement);
        }
        Ok(result)
    }
}

pub(super) fn build(root: &Path, base: Option<&str>, check: bool) -> Result<()> {
    let version=Command::new("zola").arg("--version").output().context("Zola 0.23.6 is required. Install the pinned Zola binary and add it to PATH, then rerun build_docs.")?;
    ensure!(
        version.status.success()
            && String::from_utf8_lossy(&version.stdout).trim() == format!("zola {ZOLA_VERSION}"),
        "Zola {ZOLA_VERSION} is required; found {}. Install the pinned binary and add it to PATH.",
        String::from_utf8_lossy(&version.stdout).trim()
    );
    let config = Config::load(root)?;
    let entries = history::catalog(root, &config)?;
    history::verify(root, &config, &entries)?;
    let stage = tempfile::tempdir()?;
    let stage = stage.path();
    let zola_config = read(root.join("sites/zola.toml"))?;
    let (url, prefix) = base_url(&zola_config, base)?;
    let mut zola_config = config_set(&zola_config, "", "default_language", "\"en\"");
    zola_config = config_set(&zola_config, "", "build_search_index", "true");
    zola_config = config_set(&zola_config, "languages.es", "build_search_index", "true");
    zola_config = config_set(&zola_config, "search", "index_format", "\"fuse_json\"");
    zola_config = config_set(
        &zola_config,
        "",
        "title",
        &serde_json::to_string(&config.title)?,
    );
    zola_config = config_set(
        &zola_config,
        "extra",
        "repository",
        &serde_json::to_string(&config.repository)?,
    );
    write(stage, "zola.toml", zola_config)?;
    for (path, content) in [
        (
            "templates/base.html",
            include_str!("../../site-theme/templates/base.html"),
        ),
        (
            "templates/index.html",
            include_str!("../../site-theme/templates/index.html"),
        ),
        (
            "templates/page.html",
            include_str!("../../site-theme/templates/page.html"),
        ),
        (
            "templates/section.html",
            include_str!("../../site-theme/templates/section.html"),
        ),
        (
            "static/docs.css",
            include_str!("../../site-theme/static/docs.css"),
        ),
        (
            "static/docs.js",
            include_str!("../../site-theme/static/docs.js"),
        ),
    ] {
        write(stage, path, content)?;
    }
    write(
        stage,
        "static/favicon.svg",
        include_str!("../../site-theme/static/favicon.svg"),
    )?;
    copy_tree(&root.join("sites/templates"), &stage.join("templates"))?;
    copy_tree(&root.join("sites/static"), &stage.join("static"))?;
    let head = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned());
    let development_ref = head.as_deref().unwrap_or("working-tree");
    let mut scopes = vec![];
    let mut legacy = BTreeSet::new();
    let mut root_raw = BTreeMap::new();
    for (module, label) in config.modules() {
        let mut releases = vec![None];
        releases.extend(entries.iter().filter(|e| e["module"] == module).map(Some));
        for entry in releases {
            let version = if let Some(e) = entry {
                history::route_version(e)?
            } else {
                "development".into()
            };
            let reference = entry.map(|e| string(e, "documentation_ref")).transpose()?;
            let translation = entry.and_then(|e| e["translation_tree"].as_str());
            if let Some(tree) = translation {
                let object = Command::new("git")
                    .current_dir(root)
                    .args(["cat-file", "-t", tree])
                    .output()?;
                ensure!(object.status.success() && String::from_utf8_lossy(&object.stdout).trim()=="tree",
                    "Historical translation_tree {tree} is unavailable or is not a Git tree; fetch the repository's full documentation history and translation objects, then rerun build_docs");
            }
            let source = entry
                .and_then(|e| e["source"].as_str())
                .unwrap_or(development_ref);
            let guide_paths = guides(root, reference)?;
            let mut pages: BTreeMap<String, String> =
                guide_paths.iter().map(|p| (p.clone(), slug(p))).collect();
            pages.insert("IMPORT.md".into(), "installation".into());
            pages.insert("docs/releases.md".into(), "releases".into());
            for lang in ["en", "es"] {
                let scope = format!("{prefix}{lang}/{module}/{version}/");
                let scope_index = scopes.len();
                let mut metadata = vec![];
                let mut ordered = guide_paths.clone();
                ordered.sort_by_key(|p| (!p.ends_with("/index.md"), p.clone()));
                ordered.push("IMPORT.md".into());
                ordered.push("docs/releases.md".into());
                for (page_number, path) in ordered.into_iter().enumerate() {
                    let guide = &pages[&path];
                    let english = if path == "IMPORT.md" {
                        if let Some(entry) = entry {
                            history::installation(entry, "en")?
                        } else {
                            text(root, None, &path)?
                        }
                    } else {
                        text(root, reference, &path)?
                    };
                    let (body, fallback) = if path == "IMPORT.md" && entry.is_some() {
                        (history::installation(entry.unwrap(), lang)?, false)
                    } else {
                        localized(root, reference, translation, &path, lang, &english)?
                    };
                    let rewriter = Rewriter {
                        root,
                        stage,
                        reference,
                        scope: &scope,
                        prefix: &prefix,
                        source_path: &path,
                        source_ref: source,
                        pages: &pages,
                        repository: &config.repository,
                    };
                    let raw_body = raw_anchors(&rewriter.rewrite(&body, true)?);
                    let provenance = format!("> Module: {module} · Version: {version} · Source: `{source}` · Documentation: `{}`{}\n", reference.unwrap_or(development_ref), if entry.is_none() { " · Development (unreleased)" } else { "" });
                    let fallback_notice = if fallback {
                        "> Guía en inglés para esta versión. La traducción está ausente o desactualizada.\n\n"
                    } else {
                        ""
                    };
                    let raw = format!("{provenance}\n{fallback_notice}{raw_body}");
                    let (mut title, body) =
                        title_body(&body).with_context(|| format!("Missing title in {path}"))?;
                    if guide == "installation" {
                        title = if lang == "es" {
                            "Instalación"
                        } else {
                            "Installation"
                        }
                        .into();
                    }
                    let body =
                        rewriter.rewrite(&anchors(&title_body(&english)?.1, &body), false)?;
                    let title_anchor = {
                        use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag};
                        Parser::new_ext(
                            &anchors(&english, &english),
                            Options::ENABLE_HEADING_ATTRIBUTES,
                        )
                        .find_map(|event| match event {
                            Event::Start(Tag::Heading {
                                level: HeadingLevel::H1,
                                id,
                                ..
                            }) => id.map(|s| s.into_string()),
                            _ => None,
                        })
                        .unwrap_or_default()
                    };
                    let href = route(&scope, guide);
                    let raw_href = raw_route(&scope, guide);
                    let fm=format!("+++\ntitle = {}\npath = {}\ntemplate = \"page.html\"\n[extra]\nscope_index = {scope_index}\nguide = {}\nfallback = {fallback}\ntitle_anchor = {}\n+++\n\n{body}\n",serde_json::to_string(&title)?,serde_json::to_string(href.trim_start_matches(&prefix).trim_matches('/'))?,serde_json::to_string(guide)?,serde_json::to_string(&title_anchor)?);
                    // Explicit paths are language-independent in Zola; retain the language in the path.
                    write(
                        stage,
                        &format!(
                            "content/scope-{scope_index}-page-{page_number}.{}md",
                            if lang == "es" { "es." } else { "" }
                        ),
                        fm,
                    )?;
                    write(
                        &stage.join("static"),
                        raw_href.trim_start_matches(&prefix),
                        &raw,
                    )?;
                    if path == "IMPORT.md" {
                        write(
                            &stage.join("static"),
                            &format!("{}IMPORT.md", scope.trim_start_matches(&prefix)),
                            &raw,
                        )?;
                    } else if path.starts_with("docs/agents/") {
                        write(
                            &stage.join("static"),
                            &format!(
                                "{}agents/{}",
                                scope.trim_start_matches(&prefix),
                                path.trim_start_matches("docs/agents/")
                            ),
                            &raw,
                        )?;
                    }
                    let old = if guide.is_empty() {
                        "index"
                    } else if guide == "installation" {
                        "install"
                    } else {
                        guide
                    };
                    let alias = format!("{version}/{lang}/{old}.html");
                    if legacy.insert(alias.clone()) {
                        write(&stage.join("static"),&alias,format!("<!doctype html><html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0;url={href}\"><a href=\"{href}\">{title}</a></html>"))?;
                    }
                    if lang == "en" {
                        root_raw.insert(
                            if path == "IMPORT.md" {
                                "IMPORT.md".into()
                            } else if path.starts_with("docs/agents/") {
                                format!("agents/{}", path.trim_start_matches("docs/agents/"))
                            } else {
                                "releases.md".into()
                            },
                            raw.clone(),
                        );
                    }
                    write(
                        &stage.join("static"),
                        &format!(
                            "{version}/{lang}/{}",
                            if path == "IMPORT.md" {
                                "IMPORT.md".into()
                            } else if path.starts_with("docs/agents/") {
                                format!("agents/{}", path.trim_start_matches("docs/agents/"))
                            } else {
                                "releases.md".into()
                            }
                        ),
                        raw,
                    )?;
                    metadata.push(json!({"slug":guide,"title":title,"href":href,"raw":raw_href}));
                }
                let mut scope_data = json!({"language":lang,"module":module,"label":label,"version":version,"root":scope,"source":source,"documentation_ref":reference.unwrap_or(development_ref),"development":entry.is_none(),"pages":metadata});
                if let Some(tree) = translation {
                    scope_data["translation_tree"] = json!(tree);
                }
                let mut images = vec![];
                for (name, alt) in [
                    ("catalog-light.png", "Populated light catalog"),
                    ("catalog-dark.png", "Populated dark catalog"),
                ] {
                    if let Ok(image) = bytes(root, reference, &format!("docs/screenshots/{name}")) {
                        let route = format!("media/{module}/{version}/{name}");
                        write(&stage.join("static"), &route, image)?;
                        images.push(json!({"src":format!("{prefix}{route}"),"alt":alt}));
                    }
                }
                if !images.is_empty() {
                    scope_data["images"] = json!(images);
                }
                scopes.push(scope_data);
            }
        }
    }
    for (path, body) in root_raw {
        write(&stage.join("static"), &path, body)?;
    }
    let serialized = serde_json::to_string_pretty(&scopes)?;
    write(stage, "scopes.json", &serialized)?;
    write(stage, "static/versions.json", serialized)?;
    write(
        stage,
        "content/_index.md",
        format!(
            "+++\ntitle = {}\ntemplate = \"index.html\"\n+++\n",
            serde_json::to_string(&config.title)?
        ),
    )?;
    write(
        stage,
        "content/_index.es.md",
        format!(
            "+++\ntitle = {}\ntemplate = \"index.html\"\n+++\n",
            serde_json::to_string(&config.title)?
        ),
    )?;
    write(stage,"static/llms.txt",format!("# {}\n\n- [Integration guide]({prefix}agents/index.md)\n- [Installation]({prefix}IMPORT.md)\n",config.title))?;
    let destination = root.join("sites/dist");
    let output = Command::new("zola")
        .current_dir(stage)
        .args([
            "--config",
            "zola.toml",
            "build",
            "--base-url",
            &url,
            "--output-dir",
        ])
        .arg(&destination)
        .arg("--force")
        .output()?;
    ensure!(
        output.status.success(),
        "Zola build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if check {
        validate_links(&destination, &prefix, &url)?;
    }
    Ok(())
}

fn validate_links(destination: &Path, prefix: &str, url: &str) -> Result<()> {
    fn collect(root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for p in children(root)? {
            if p.is_dir() {
                collect(&p, out)?
            } else if p.extension().is_some_and(|e| e == "html") {
                out.push(p)
            }
        }
        Ok(())
    }
    let mut files = vec![];
    collect(destination, &mut files)?;
    let attrs = Regex::new(r#"(?:href|src)\s*=\s*["']([^"']*)["']"#)?;
    let ids = Regex::new(r#"\bid\s*=\s*["']([^"']+)["']"#)?;
    let mut failures = vec![];
    for file in files {
        let html = fs::read_to_string(&file)?;
        for c in attrs.captures_iter(&html) {
            let target = c[1].replace("&amp;", "&");
            let target = if let Some(t) = target.strip_prefix(url) {
                format!("{prefix}{t}")
            } else {
                target
            };
            if target.is_empty()
                || target.starts_with("//")
                || target.split('#').next().unwrap_or("").contains(':')
            {
                continue;
            }
            let (path, fragment) = target.split_once('#').unwrap_or((&target, ""));
            let path = path.split('?').next().unwrap_or(path);
            let resolved = if path.is_empty() {
                file.clone()
            } else if path.starts_with('/') {
                if let Some(local) = path.strip_prefix(prefix) {
                    destination.join(local)
                } else {
                    failures.push(format!(
                        "{}: {target} is outside site base {prefix}",
                        file.display()
                    ));
                    continue;
                }
            } else {
                let parent = file.parent().unwrap().strip_prefix(destination)?;
                destination.join(normalize(&format!("{}/{path}", parent.display())))
            };
            let resolved = if resolved.is_dir() || path.ends_with('/') {
                resolved.join("index.html")
            } else {
                resolved
            };
            if !resolved.is_file() {
                failures.push(format!("{}: missing {target}", file.display()));
            } else if !fragment.is_empty() && resolved.extension().is_some_and(|e| e == "html") {
                let fragment = percent_encoding::percent_decode_str(fragment).decode_utf8_lossy();
                let page = fs::read_to_string(&resolved)?;
                if !ids.captures_iter(&page).any(|id| id[1] == *fragment) {
                    failures.push(format!("{}: missing anchor {target}", file.display()));
                }
            }
        }
    }
    if !failures.is_empty() {
        bail!(
            "Generated documentation has broken local links:\n{}",
            failures.join("\n")
        );
    }
    Ok(())
}
