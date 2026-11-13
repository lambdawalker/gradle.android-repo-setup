//! Shared immutable documentation content staging for bundled site renderers.
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

#[derive(Clone, Default, serde::Deserialize)]
pub(crate) struct Settings {
    #[serde(default)]
    pub pages: BTreeMap<String, String>,
    #[serde(default)]
    pub current_pages: BTreeMap<String, String>,
    pub translation_manifest: Option<String>,
    #[serde(default)]
    pub translation_paths: BTreeMap<String, String>,
    #[serde(default)]
    pub aliases: Vec<Alias>,
}
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Alias {
    pub source: String,
    pub html: Option<String>,
    pub raw: Option<String>,
}
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        })
}
fn alias_path(pattern: &str, language: &str, module: &str, version: &str) -> Result<String> {
    let path = pattern
        .replace("{language}", language)
        .replace("{module}", module)
        .replace("{version}", version);
    ensure!(safe_path(&path), "Unsafe site alias output path: {pattern}");
    Ok(path)
}
impl Settings {
    pub(crate) fn load(config: &Config) -> Result<Self> {
        let settings: Self =
            serde_json::from_value(config.site.clone().unwrap_or_else(|| json!({})))
                .context("Invalid site content settings")?;
        settings.validate()?;
        Ok(settings)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        for (source, slug) in self.pages.iter().chain(&self.current_pages) {
            ensure!(
                safe_path(source) && source.ends_with(".md"),
                "site.pages source must be a safe repository Markdown path: {source}"
            );
            ensure!(
                slug.is_empty() || safe_path(slug),
                "site.pages slug must be an empty overview slug or safe path: {slug}"
            );
        }
        if let Some(path) = &self.translation_manifest {
            ensure!(
                safe_path(path),
                "site.translation_manifest must be a safe repository path"
            );
        }
        let mut translated = BTreeSet::new();
        for (source, path) in &self.translation_paths {
            ensure!(
                safe_path(source)
                    && source.ends_with(".md")
                    && safe_path(path)
                    && path.ends_with(".md"),
                "site.translation_paths must map safe repository Markdown paths"
            );
            ensure!(
                translated.insert(path),
                "Duplicate translated source path: {path}"
            );
        }
        for alias in &self.aliases {
            ensure!(
                safe_path(&alias.source) && alias.source.ends_with(".md"),
                "site.aliases source must be a safe repository Markdown path"
            );
            ensure!(
                alias.html.is_some() || alias.raw.is_some(),
                "site.aliases requires html or raw"
            );
            for pattern in [&alias.html, &alias.raw].into_iter().flatten() {
                alias_path(pattern, "en", "module", "development")?;
            }
            if let Some(path) = &alias.html {
                ensure!(
                    path.ends_with(".html"),
                    "HTML site aliases must end in .html"
                );
            }
            if let Some(path) = &alias.raw {
                ensure!(path.ends_with(".md"), "Raw site aliases must end in .md");
            }
        }
        Ok(())
    }
    fn resolved_translation_path(
        &self,
        root: &Path,
        reference: Option<&str>,
        translation: Option<&str>,
        path: &str,
    ) -> Result<String> {
        if let Some(path) = self.translation_paths.get(path) {
            return Ok(path.clone());
        }
        let configured = self
            .translation_manifest
            .as_deref()
            .unwrap_or("docs/es/translations.json");
        let tr = translation.or(reference);
        let manifest = text(root, tr, configured).or_else(|error| {
            if translation.is_some() && self.translation_manifest.is_none() {
                text(root, tr, "translations.json")
            } else {
                Err(error)
            }
        });
        if let Ok(manifest) = manifest {
            let manifest: Value = serde_json::from_str(&manifest)
                .context("Invalid Spanish translation hash manifest")?;
            if let Some(translated) = manifest
                .get(path)
                .or_else(|| manifest.get(translation_key(path)))
                .and_then(|record| record.get("path"))
                .and_then(Value::as_str)
            {
                ensure!(
                    safe_path(translated),
                    "Unsafe translated repository path: {translated}"
                );
                return Ok(translated.into());
            }
        }
        Ok(self.translated_path(path))
    }
    fn translated_path(&self, path: &str) -> String {
        self.translation_paths
            .get(path)
            .cloned()
            .unwrap_or_else(|| format!("docs/es/{}", translation_key(path)))
    }
}
fn insert_link_mapping(
    links: &mut BTreeMap<String, String>,
    path: &str,
    guide: &str,
) -> Result<()> {
    ensure!(
        links.get(path).is_none_or(|existing| existing == guide),
        "Translated source path conflicts with another page: {path}"
    );
    links.insert(path.into(), guide.into());
    Ok(())
}
fn translation_key(path: &str) -> &str {
    path.strip_prefix("docs/agents/")
        .unwrap_or(path.strip_prefix("docs/").unwrap_or(path))
}

pub(super) fn write(root: &Path, path: &str, bytes: impl AsRef<[u8]>) -> Result<()> {
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
fn exists_at(root: &Path, reference: &str, path: &str) -> bool {
    Command::new("git")
        .current_dir(root)
        .args(["cat-file", "-e", &format!("{reference}:{path}")])
        .output()
        .is_ok_and(|output| output.status.success())
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
pub(super) fn copy_tree(from: &Path, to: &Path) -> Result<()> {
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
struct Localized {
    body: String,
    fallback: bool,
    source_path: String,
    reference: Option<String>,
    translation_subtree: bool,
}
fn code_fences(markdown: &str) -> Vec<(String, String)> {
    use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
    let mut result = vec![];
    let mut active = None;
    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(language))) => {
                active = Some((language.into_string(), String::new()))
            }
            Event::Text(text) => {
                if let Some((_, body)) = &mut active {
                    body.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(block) = active.take() {
                    result.push(block);
                }
            }
            _ => {}
        }
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
    settings: &Settings,
) -> Result<Localized> {
    let fallback = || Localized {
        body: english.into(),
        fallback: lang != "en",
        source_path: path.into(),
        reference: reference.map(str::to_owned),
        translation_subtree: false,
    };
    if lang == "en" {
        return Ok(fallback());
    }
    let key = translation_key(path);
    let tr = translation.or(reference);
    let manifest_path = settings
        .translation_manifest
        .as_deref()
        .unwrap_or("docs/es/translations.json");
    let manifest = text(root, tr, manifest_path).or_else(|error| {
        if translation.is_some() && settings.translation_manifest.is_none() {
            text(root, tr, "translations.json")
        } else {
            Err(error)
        }
    });
    if let Ok(manifest) = manifest {
        let manifest: Value =
            serde_json::from_str(&manifest).context("Invalid Spanish translation hash manifest")?;
        let record = manifest
            .get(path)
            .or_else(|| manifest.get(key))
            .unwrap_or(&Value::Null);
        let hash = format!("{:x}", Sha256::digest(english.as_bytes()));
        let expected = record.as_str().or_else(|| record["source_sha256"].as_str());
        if expected != Some(&hash) {
            return Ok(fallback());
        }
        let translated_path = settings
            .translation_paths
            .get(path)
            .map(String::as_str)
            .or_else(|| record["path"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| settings.translated_path(path));
        ensure!(
            safe_path(&translated_path),
            "Unsafe translated repository path: {translated_path}"
        );
        let translated = text(root, tr, &translated_path)
            .map(|body| (body, false))
            .or_else(|error| {
                if translation.is_some() && settings.translation_paths.get(path).is_none() {
                    text(root, tr, key).map(|body| (body, true))
                } else {
                    Err(error)
                }
            });
        if let Ok((translated, translation_subtree)) = translated {
            ensure!(
                code_fences(english) == code_fences(&translated),
                "Translated executable code fences differ from canonical source: {path}"
            );
            return Ok(Localized {
                body: anchors(english, &translated),
                fallback: false,
                source_path: translated_path,
                reference: tr.map(str::to_owned),
                translation_subtree,
            });
        }
    }
    Ok(fallback())
}
fn base_url(config: &str, base: Option<&str>) -> Result<(String, String)> {
    let regex = Regex::new(r#"(?m)^\s*base_url\s*=\s*["']([^"']+)["']"#)?;
    let configured = regex
        .captures(config)
        .context("sites/zola.toml needs an absolute base_url")?[1]
        .to_owned();
    let (scheme, rest) = configured
        .split_once("://")
        .context("Documentation base_url must include https:// or http://")?;
    let (host, _path) = rest.split_once('/').unwrap_or((rest, ""));
    ensure!(
        matches!(scheme, "https" | "http") && !host.is_empty(),
        "Documentation base_url must use http:// or https:// and include a host"
    );
    if let Some(base) = base {
        ensure!(
            !base.starts_with("//"),
            "Documentation --base must be an absolute URL or path prefix"
        );
    }
    let url = if let Some(base) = base {
        if base.contains("://") {
            base.to_owned()
        } else {
            format!("{scheme}://{host}/{}", base.trim_matches('/'))
        }
    } else {
        configured
    };
    let (scheme, rest) = url
        .split_once("://")
        .context("Documentation base_url must be absolute")?;
    ensure!(
        matches!(scheme, "http" | "https") && !rest.is_empty() && !rest.starts_with('/'),
        "Documentation base_url must use http:// or https:// and include a host"
    );
    ensure!(
        !url.contains(['\\', '?', '#', '\0']) && !url.chars().any(char::is_whitespace),
        "Documentation base URL cannot contain query, fragment, whitespace or backslash characters"
    );
    let path = rest.split_once('/').map(|(_, p)| p).unwrap_or("");
    ensure!(
        !path.split('/').any(|part| part == ".." || part == "."),
        "Documentation base URL cannot contain parent or current directory segments"
    );
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
    documentation_ref: Option<&'a str>,
    scope: &'a str,
    prefix: &'a str,
    source_path: &'a str,
    canonical_source_path: &'a str,
    translation_subtree: bool,
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
            let canonical_parent = self
                .canonical_source_path
                .rsplit_once('/')
                .map(|(p, _)| p)
                .unwrap_or("");
            let canonical_page = normalize(&format!("{canonical_parent}/{path}"));
            let href = if let Some(guide) = self
                .pages
                .get(&resolved)
                .or_else(|| self.pages.get(&canonical_page))
            {
                if raw {
                    raw_route(self.scope, guide)
                } else {
                    route(self.scope, guide)
                }
            } else if image {
                // Keep logical repository paths for Markdown resolution. Only
                // Git reads strip the retained docs/es translation subtree root.
                let translated = if self.translation_subtree {
                    resolved
                        .strip_prefix("docs/es/")
                        .map(|path| bytes(self.root, self.reference, path))
                        .unwrap_or_else(|| {
                            Err(anyhow::anyhow!(
                                "Asset is outside retained translation subtree"
                            ))
                        })
                } else {
                    bytes(self.root, self.reference, &resolved)
                };
                let image=translated.or_else(|original|bytes(self.root,self.documentation_ref,&resolved)
                    .with_context(||format!("Missing documentation image {resolved} in immutable documentation snapshot: {original}")))?;
                let asset_path = resolved;
                let media = format!("media/{asset_path}");
                write(
                    &self.stage.join("static"),
                    &format!("{}{media}", self.scope.trim_start_matches(self.prefix)),
                    image,
                )?;
                format!("{}{media}", self.scope)
            } else {
                let reference = if exists_at(self.root, self.source_ref, &resolved) {
                    self.source_ref
                } else if let Some(documentation) = self
                    .documentation_ref
                    .filter(|r| exists_at(self.root, r, &resolved))
                {
                    documentation
                } else {
                    self.source_ref
                };
                format!(
                    "https://github.com/{}/blob/{}/{}",
                    self.repository, reference, resolved
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

pub(super) struct PreparedSite {
    pub url: String,
    pub prefix: String,
}

pub(super) fn prepare(
    root: &Path,
    stage: &Path,
    config: &Config,
    base: Option<&str>,
    configured_url: Option<&str>,
) -> Result<PreparedSite> {
    let options = Settings::load(config)?;
    let entries = history::catalog(root, config)?;
    history::verify(root, config, &entries)?;
    let legacy = root.join("sites/zola.toml");
    let legacy_config = if legacy.is_file() {
        Some(read(legacy)?)
    } else {
        None
    };
    let inferred = {
        let (owner, repository) = config.repository.split_once('/').unwrap();
        format!("https://{owner}.github.io/{repository}/")
    };
    let settings = if let Some(url) = configured_url {
        format!("base_url = {}", serde_json::to_string(url)?)
    } else {
        legacy_config.unwrap_or(format!("base_url = {}", serde_json::to_string(&inferred)?))
    };
    let (url, prefix) = base_url(&settings, base)?;
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
    let mut manifest_pages = vec![];
    let mut legacy = BTreeSet::new();
    let mut root_raw = BTreeMap::new();
    let mut configured_aliases = BTreeMap::new();
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
            if entry.is_some() {
                let available = Command::new("git")
                    .current_dir(root)
                    .args(["cat-file", "-e", &format!("{source}^{{commit}}")])
                    .output()?;
                ensure!(available.status.success(),"Immutable publication source {source} is unavailable; fetch the repository's full source and documentation history, then rerun build_docs");
            }
            let guide_paths = guides(root, reference)?;
            let mut pages: BTreeMap<String, String> =
                guide_paths.iter().map(|p| (p.clone(), slug(p))).collect();
            pages.insert("IMPORT.md".into(), "installation".into());
            pages.insert("docs/releases.md".into(), "releases".into());
            pages.extend(options.pages.clone());
            pages.extend(options.current_pages.clone());
            let mut slugs = BTreeSet::new();
            for (path, guide) in &pages {
                ensure!(
                    slugs.insert(guide),
                    "Duplicate site page route {guide}: {path}; remap it through site.pages"
                );
            }
            for path in options.translation_paths.keys() {
                ensure!(
                    pages.contains_key(path),
                    "Unknown translation source page: {path}"
                );
            }
            for alias in &options.aliases {
                ensure!(
                    pages.contains_key(&alias.source),
                    "Unknown site alias source page: {}",
                    alias.source
                );
            }
            let mut links = pages.clone();
            for (path, guide) in &pages {
                let current = options.current_pages.contains_key(path);
                let translated = options.resolved_translation_path(
                    root,
                    if current { None } else { reference },
                    if current { None } else { translation },
                    path,
                )?;
                insert_link_mapping(&mut links, &translated, guide)?;
            }
            for lang in ["en", "es"] {
                let scope = format!("{prefix}{lang}/{module}/{version}/");
                let scope_index = scopes.len();
                let mut metadata = vec![];
                let mut ordered: Vec<_> = pages.keys().cloned().collect();
                ordered.sort_by_key(|p| (!pages[p].is_empty(), p.clone()));
                for (page_number, path) in ordered.into_iter().enumerate() {
                    let guide = &pages[&path];
                    let current = options.current_pages.contains_key(&path);
                    let page_reference = if current { None } else { reference };
                    let page_source = if current { development_ref } else { source };
                    let english = if path == "IMPORT.md" && !current {
                        if let Some(entry) = entry {
                            history::installation(entry, "en")?
                        } else {
                            text(root, None, &path)?
                        }
                    } else {
                        text(root, page_reference, &path)?
                    };
                    let localized = if path == "IMPORT.md" && entry.is_some() && !current {
                        Localized {
                            body: history::installation(entry.unwrap(), lang)?,
                            fallback: false,
                            source_path: path.clone(),
                            reference: reference.map(str::to_owned),
                            translation_subtree: false,
                        }
                    } else {
                        localized(
                            root,
                            page_reference,
                            if current { None } else { translation },
                            &path,
                            lang,
                            &english,
                            &options,
                        )?
                    };
                    let body = &localized.body;
                    let fallback = localized.fallback;
                    let rewriter = Rewriter {
                        root,
                        stage,
                        reference: localized.reference.as_deref(),
                        documentation_ref: page_reference,
                        scope: &scope,
                        prefix: &prefix,
                        source_path: &localized.source_path,
                        canonical_source_path: &path,
                        translation_subtree: localized.translation_subtree,
                        source_ref: page_source,
                        pages: &links,
                        repository: &config.repository,
                    };
                    let raw_body = raw_anchors(&rewriter.rewrite(&body, true)?);
                    let provenance = format!("> Module: {module} · Version: {version} · Source: `{page_source}` · Documentation: `{}`{}\n", page_reference.unwrap_or(development_ref), if entry.is_none() { " · Development (unreleased)" } else { "" });
                    let fallback_notice = if fallback {
                        "> Guía en inglés para esta versión. La traducción está ausente o desactualizada.\n\n"
                    } else {
                        ""
                    };
                    let correction = if current {
                        if lang == "es" {
                            format!("> Recurso compartido actual del proyecto: contenido de `{development_ref}`, no documentación inmutable de esta publicación.\n\n")
                        } else {
                            format!("> Current shared project resource: content from `{development_ref}`, not immutable documentation of this publication.\n\n")
                        }
                    } else if entry.is_some()
                        && reference.is_some_and(|r| r != source)
                        && path != "IMPORT.md"
                    {
                        if lang == "es" {
                            format!("> Corrección posterior a la publicación: la documentación y los ejemplos ejecutables pertenecen a `{}`; ejecute los ejemplos en esa revisión. La publicación original permanece en `{source}`.\n\n",reference.unwrap())
                        } else {
                            format!("> Postrelease documentation correction: documentation and runnable examples belong to `{}`; run examples at that revision. The original publication remains at `{source}`.\n\n",reference.unwrap())
                        }
                    } else {
                        String::new()
                    };
                    let raw = format!("{provenance}\n{correction}{fallback_notice}{raw_body}");
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
                    let body = format!(
                        "{correction}{}",
                        rewriter.rewrite(&anchors(&title_body(&english)?.1, &body), false)?
                    );
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
                    manifest_pages.push(json!({"path":href.trim_start_matches(&prefix).trim_matches('/'),"title":title,"body":body,"scope_index":scope_index,"guide":guide,"fallback":fallback,"title_anchor":title_anchor,"current_resource":current,"source":page_source,"documentation_ref":page_reference.unwrap_or(development_ref)}));
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
                    let legacy_raw = if path == "IMPORT.md" {
                        Some("IMPORT.md".to_owned())
                    } else if path.starts_with("docs/agents/") {
                        Some(format!(
                            "agents/{}",
                            path.trim_start_matches("docs/agents/")
                        ))
                    } else if path == "docs/releases.md" {
                        Some("releases.md".to_owned())
                    } else {
                        None
                    };
                    if let Some(legacy_raw) = legacy_raw {
                        if lang == "en" && entry.is_none() {
                            root_raw.insert(legacy_raw.clone(), raw.clone());
                        }
                        write(
                            &stage.join("static"),
                            &format!("{version}/{lang}/{legacy_raw}"),
                            &raw,
                        )?;
                    }
                    for alias in options.aliases.iter().filter(|a| a.source == path) {
                        for (pattern, human) in [(&alias.html, true), (&alias.raw, false)] {
                            if let Some(pattern) = pattern {
                                let output = alias_path(pattern, lang, module, &version)?;
                                let html = if human {
                                    format!("<!doctype html><html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0;url={href}\"><a href=\"{href}\">Documentation</a></html>")
                                } else {
                                    raw.clone()
                                };
                                ensure!(
                                    configured_aliases.insert(output.clone(), html).is_none(),
                                    "Duplicate configured site alias: {output}"
                                );
                            }
                        }
                    }
                    metadata.push(json!({"slug":guide,"title":title,"href":href,"raw":raw_href}));
                }
                let mut scope_data = json!({"language":lang,"module":module,"label":label,"version":version,"root":scope,"source":source,"documentation_ref":reference.unwrap_or(development_ref),"development":entry.is_none(),"pages":metadata});
                if let Some(entry) = entry {
                    scope_data["release"] = entry.clone();
                    scope_data["destinations"] = json!(entry["destinations"]
                        .as_object()
                        .context("Invalid archive destinations")?
                        .keys()
                        .collect::<Vec<_>>());
                }
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
    for (path, body) in configured_aliases {
        let canonical_page = manifest_pages
            .iter()
            .any(|page| path == format!("{}/index.html", page["path"].as_str().unwrap()));
        ensure!(
            !canonical_page
                && !stage.join("static").join(&path).exists()
                && !path.starts_with("pagefind/")
                && !path.starts_with("_astro/")
                && !matches!(
                    path.as_str(),
                    "index.html"
                        | "404.html"
                        | "en/index.html"
                        | "es/index.html"
                        | "versions.json"
                        | "llms.txt"
                        | "scopes.json"
                        | "pages.json"
                        | "site.json"
                ),
            "Configured site alias collides with a canonical output: {path}"
        );
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
    write(
        stage,
        "pages.json",
        serde_json::to_vec_pretty(&manifest_pages)?,
    )?;
    write(
        stage,
        "site.json",
        serde_json::to_vec_pretty(
            &json!({"title":config.title,"repository":config.repository,"base_url":url,"prefix":prefix}),
        )?,
    )?;
    Ok(PreparedSite { url, prefix })
}

pub(super) fn validate_links(destination: &Path, prefix: &str, url: &str) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_url_precedes_legacy_url_and_repository_default() {
        let root = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir().unwrap();
        write(
            root.path(),
            ".project/docs/config.yaml",
            "schema: 1\nrepository: example/library\ntitle: Reference\ntooling_ref: v0.1.20\n",
        )
        .unwrap();
        let config = Config::load(root.path()).unwrap();
        assert_eq!(
            prepare(root.path(), stage.path(), &config, None, None)
                .unwrap()
                .url,
            "https://example.github.io/library/"
        );
        write(
            root.path(),
            "sites/zola.toml",
            "base_url = 'https://legacy.example/reference/'\n",
        )
        .unwrap();
        assert_eq!(
            prepare(root.path(), stage.path(), &config, None, None)
                .unwrap()
                .url,
            "https://legacy.example/reference/"
        );
        let explicit = prepare(
            root.path(),
            stage.path(),
            &config,
            Some("/preview"),
            Some("https://current.example/custom/"),
        )
        .unwrap();
        assert_eq!(explicit.url, "https://current.example/preview/");
        assert_eq!(explicit.prefix, "/preview/");
    }
    #[test]
    fn rejects_unsafe_deployment_urls_and_prefixes() {
        for url in [
            "ftp://example.com/docs",
            "https://example.com/docs?query=yes",
            "https://example.com/docs#anchor",
        ] {
            assert!(
                base_url(&format!("base_url = {url:?}"), None).is_err(),
                "{url}"
            );
        }
        for base in [
            "//other.example/docs",
            "/../outside",
            "/docs?query=yes",
            "/docs#anchor",
            "/docs\\escape",
        ] {
            assert!(
                base_url("base_url = \"https://example.com/docs/\"", Some(base)).is_err(),
                "{base}"
            );
        }
    }
    #[test]
    fn shared_manifests_preserve_scoped_links_raw_files_and_provenance() {
        let root = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir().unwrap();
        write(root.path(), ".project/docs/config.yaml", "schema: 1\nrepository: example/library\ntitle: Reference\ntooling_ref: v0.1.20\nmodules:\n  api: API\n").unwrap();
        write(
            root.path(),
            "docs/agents/index.md",
            "# Overview\n\n[Quickstart](quickstart.md#setup)\n",
        )
        .unwrap();
        write(
            root.path(),
            "docs/agents/quickstart.md",
            "# Quickstart\n\n## Setup\n\n![Icon](icon.svg)\n",
        )
        .unwrap();
        write(root.path(), "docs/agents/icon.svg", "<svg/>").unwrap();
        write(root.path(), "docs/releases.md", "# Releases\n").unwrap();
        write(root.path(), "IMPORT.md", "# Installation\n").unwrap();
        let config = Config::load(root.path()).unwrap();
        let prepared =
            prepare(root.path(), stage.path(), &config, Some("/reference"), None).unwrap();
        assert_eq!(prepared.url, "https://example.github.io/reference/");
        let pages: Value =
            serde_json::from_slice(&fs::read(stage.path().join("pages.json")).unwrap()).unwrap();
        assert_eq!(pages[0]["path"], "en/api/development");
        assert!(pages[0]["body"]
            .as_str()
            .unwrap()
            .contains("/reference/en/api/development/quickstart/#setup"));
        let quickstart = pages
            .as_array()
            .unwrap()
            .iter()
            .find(|page| page["path"] == "en/api/development/quickstart")
            .unwrap();
        assert!(quickstart["body"].as_str().unwrap().contains("{#setup}"));
        assert!(stage
            .path()
            .join("static/en/api/development/media/docs/agents/icon.svg")
            .is_file());
        let raw = fs::read_to_string(stage.path().join("static/en/api/development/raw/index.md"))
            .unwrap();
        assert!(raw.contains("Development (unreleased)"));
        assert!(raw.contains("/reference/en/api/development/raw/quickstart.md#setup"));
        assert!(stage
            .path()
            .join("static/development/en/index.html")
            .is_file());
        assert!(pages
            .as_array()
            .unwrap()
            .iter()
            .any(|page| page["path"] == "es/api/development" && page["fallback"] == true));
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    fn fixture(settings: &str) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), ".project/docs/config.yaml", format!("schema: 1\nrepository: example/library\ntitle: Reference\ntooling_ref: v0.1.21\nmodules:\n  api: API\n{settings}")).unwrap();
        write(temp.path(), "docs/agents/index.md", "# Overview\n").unwrap();
        write(temp.path(), "IMPORT.md", "# Installation\n").unwrap();
        write(temp.path(), "docs/releases.md", "# Releases\n").unwrap();
        temp
    }
    #[test]
    fn configured_pages_translate_from_actual_paths_and_preserve_aliases() {
        let root = fixture("site:\n  pages:\n    README.md: project\n  translation_manifest: sites/translations.json\n  translation_paths:\n    README.md: docs/es/README.md\n    docs/agents/index.md: docs/es/agents/index.md\n  aliases:\n    - source: README.md\n      html: '{language}/main/README.html'\n      raw: 'raw/{language}/main/README.md'\n");
        let english =
            "# Project\n\n[Overview](docs/agents/index.md)\n\n```kotlin\nval stable = true\n```\n";
        write(root.path(), "README.md", english).unwrap();
        write(
            root.path(),
            "docs/es/README.md",
            "# Proyecto\n\n[Resumen](agents/index.md)\n\n```kotlin\nval stable = true\n```\n",
        )
        .unwrap();
        let manifest = json!({"README.md":{"path":"docs/es/README.md","source_sha256":format!("{:x}",Sha256::digest(english.as_bytes())),"provenance":"Reviewed translation"}});
        write(root.path(), "sites/translations.json", manifest.to_string()).unwrap();
        let config = Config::load(root.path()).unwrap();
        let stage = tempfile::tempdir().unwrap();
        prepare(root.path(), stage.path(), &config, Some("/reference"), None).unwrap();
        let pages: Value =
            serde_json::from_slice(&fs::read(stage.path().join("pages.json")).unwrap()).unwrap();
        let spanish = pages
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["path"] == "es/api/development/project")
            .unwrap();
        assert_eq!(spanish["fallback"], false);
        assert!(spanish["body"]
            .as_str()
            .unwrap()
            .contains("[Resumen](/reference/es/api/development/)"));
        assert!(
            fs::read_to_string(stage.path().join("static/es/main/README.html"))
                .unwrap()
                .contains("/reference/es/api/development/project/")
        );
        assert_eq!(
            fs::read(stage.path().join("static/raw/es/main/README.md")).unwrap(),
            fs::read(
                stage
                    .path()
                    .join("static/es/api/development/raw/project.md")
            )
            .unwrap()
        );
    }
    #[test]
    fn configured_aliases_reject_canonical_metadata_and_unsafe_routes() {
        for raw in [
            "versions.json",
            "../escape.md",
            "en/api/development/raw/index.md",
        ] {
            let root = fixture(&format!(
                "site:\n  aliases:\n    - source: docs/agents/index.md\n      raw: '{raw}'\n"
            ));
            let result = Config::load(root.path()).and_then(|config| {
                prepare(
                    root.path(),
                    tempfile::tempdir().unwrap().path(),
                    &config,
                    None,
                    None,
                )
            });
            assert!(result.is_err(), "{raw}");
        }
    }
    #[test]
    fn source_links_prefer_publication_source_then_documentation_revision() {
        let root = fixture("");
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(root.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        write(root.path(), "library/Published.kt", "class Published").unwrap();
        git(&["init"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "commit",
            "-m",
            "release",
        ]);
        let source = git(&["rev-parse", "HEAD"]);
        write(
            root.path(),
            "library/src/test/kotlin/DocumentationExamplesTest.kt",
            "class DocumentationExamplesTest",
        )
        .unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "commit",
            "-m",
            "examples",
        ]);
        let documentation = git(&["rev-parse", "HEAD"]);
        let stage = tempfile::tempdir().unwrap();
        let pages = BTreeMap::new();
        let rewriter = Rewriter {
            root: root.path(),
            stage: stage.path(),
            reference: Some(&documentation),
            documentation_ref: Some(&documentation),
            scope: "/en/api/1.0.0/",
            prefix: "/",
            source_path: "docs/agents/quickstart.md",
            canonical_source_path: "docs/agents/quickstart.md",
            translation_subtree: false,
            source_ref: &source,
            pages: &pages,
            repository: "example/library",
        };
        let input="[Examples](../../library/src/test/kotlin/DocumentationExamplesTest.kt) [Published](../../library/Published.kt)";
        for raw in [false, true] {
            let output = rewriter.rewrite(input, raw).unwrap();
            assert!(output.contains(&format!(
                "/blob/{documentation}/library/src/test/kotlin/DocumentationExamplesTest.kt"
            )));
            assert!(output.contains(&format!("/blob/{source}/library/Published.kt")));
        }
    }
    #[test]
    fn fresh_translations_reject_changed_executable_fences() {
        let root = fixture("");
        let english = "# Overview\n\n```kotlin\nval verified = true\n```\n";
        write(root.path(), "docs/agents/index.md", english).unwrap();
        write(
            root.path(),
            "docs/es/index.md",
            "# Resumen\n\n```kotlin\nval verified = false\n```\n",
        )
        .unwrap();
        write(
            root.path(),
            "docs/es/translations.json",
            json!({"index.md":format!("{:x}",Sha256::digest(english.as_bytes()))}).to_string(),
        )
        .unwrap();
        let error = prepare(
            root.path(),
            tempfile::tempdir().unwrap().path(),
            &Config::load(root.path()).unwrap(),
            None,
            None,
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("code fences"));
    }
    #[test]
    fn current_pages_and_root_raw_remain_current_inside_release_scopes() {
        let root=fixture("site:\n  current_pages:\n    docs/maintenance.md: project/development\n  pages:\n    README.md: project/readme\n");
        write(root.path(), "README.md", "# Project\n").unwrap();
        write(
            root.path(),
            "docs/maintenance.md",
            "# Maintenance\n\nOld maintenance.\n",
        )
        .unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(root.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "commit",
            "-m",
            "release",
        ]);
        let source = git(&["rev-parse", "HEAD"]);
        let record = json!({"module":"api","version":"1.2.3","source":source,"group":"com.example","artifact":"api","phase":"confirmed-public","repository":"maven-central","source_tag":"api/v1.2.3-maven-central"});
        let archive = history::archive_record(root.path(), &record).unwrap();
        write(root.path(), "docs/agents/index.md", "# Current overview\n").unwrap();
        write(
            root.path(),
            "docs/maintenance.md",
            "# Maintenance\n\nCurrent maintenance.\n",
        )
        .unwrap();
        let archived = fs::read(root.path().join(&archive)).unwrap();
        let config = Config::load(root.path()).unwrap();
        let stage = tempfile::tempdir().unwrap();
        prepare(root.path(), stage.path(), &config, None, None).unwrap();
        let raw = fs::read_to_string(stage.path().join("static/agents/index.md")).unwrap();
        assert!(raw.contains("Development (unreleased)"));
        assert!(raw.contains("Current overview"));
        let releases = fs::read_to_string(stage.path().join("static/releases.md")).unwrap();
        assert!(releases.contains("# Releases"));
        assert!(!releases.contains("# Project"));
        let pages: Value =
            serde_json::from_slice(&fs::read(stage.path().join("pages.json")).unwrap()).unwrap();
        let current = pages
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["path"] == "en/api/1.2.3-maven-central/project/development")
            .unwrap();
        assert_eq!(current["current_resource"], true);
        assert!(current["body"]
            .as_str()
            .unwrap()
            .contains("Current maintenance."));
        assert!(current["body"]
            .as_str()
            .unwrap()
            .contains("Current shared project resource"));
        let scopes: Value =
            serde_json::from_slice(&fs::read(stage.path().join("scopes.json")).unwrap()).unwrap();
        let released = scopes
            .as_array()
            .unwrap()
            .iter()
            .find(|scope| scope["version"] == "1.2.3-maven-central")
            .unwrap();
        assert_eq!(released["release"]["destinations"]["maven-central"], record);
        assert_eq!(archived, fs::read(root.path().join(archive)).unwrap());
    }
    #[test]
    fn configured_aliases_cannot_replace_language_homes() {
        let root=fixture("site:\n  aliases:\n    - source: docs/agents/index.md\n      html: '{language}/index.html'\n");
        assert!(prepare(
            root.path(),
            tempfile::tempdir().unwrap().path(),
            &Config::load(root.path()).unwrap(),
            None,
            None
        )
        .is_err());
    }
    #[test]
    fn rich_manifest_paths_alone_map_links_between_translated_pages() {
        let root=fixture("site:\n  pages:\n    README.md: project\n  translation_manifest: sites/translations.json\n");
        let overview = "# Overview\n\n[Project](../../README.md)\n";
        let project = "# Project\n";
        write(root.path(), "docs/agents/index.md", overview).unwrap();
        write(root.path(), "README.md", project).unwrap();
        write(
            root.path(),
            "translations/overview.md",
            "# Resumen\n\n[Proyecto](project.md)\n",
        )
        .unwrap();
        write(root.path(), "translations/project.md", "# Proyecto\n").unwrap();
        write(root.path(),"sites/translations.json",json!({"docs/agents/index.md":{"path":"translations/overview.md","source_sha256":format!("{:x}",Sha256::digest(overview.as_bytes()))},"README.md":{"path":"translations/project.md","source_sha256":format!("{:x}",Sha256::digest(project.as_bytes()))}}).to_string()).unwrap();
        let stage = tempfile::tempdir().unwrap();
        prepare(
            root.path(),
            stage.path(),
            &Config::load(root.path()).unwrap(),
            None,
            None,
        )
        .unwrap();
        let pages: Value =
            serde_json::from_slice(&fs::read(stage.path().join("pages.json")).unwrap()).unwrap();
        let spanish = pages
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["path"] == "es/api/development")
            .unwrap();
        assert!(spanish["body"]
            .as_str()
            .unwrap()
            .contains("[Proyecto](/library/es/api/development/project/)"));
    }
    #[test]
    fn translated_path_cannot_replace_another_canonical_guide() {
        let root=fixture("site:\n  pages:\n    README.md: project\n  translation_paths:\n    docs/agents/index.md: README.md\n");
        write(root.path(), "README.md", "# Project\n").unwrap();
        assert!(prepare(
            root.path(),
            tempfile::tempdir().unwrap().path(),
            &Config::load(root.path()).unwrap(),
            None,
            None
        )
        .is_err());
    }
    #[test]
    fn missing_publication_source_is_not_treated_as_an_absent_example() {
        let root = fixture("");
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(root.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success());
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "commit",
            "-m",
            "docs",
        ]);
        let documentation = git(&["rev-parse", "HEAD"]);
        let record = json!({"module":"api","version":"1.2.3","source":"a".repeat(40),"group":"com.example","artifact":"api","phase":"confirmed-public","repository":"maven-central","source_tag":"api/v1.2.3-maven-central"});
        let archive = history::archive_record(root.path(), &record).unwrap();
        let mut value: Value =
            serde_json::from_slice(&fs::read(root.path().join(&archive)).unwrap()).unwrap();
        value["documentation_ref"] = json!(documentation);
        write(root.path(), archive.to_str().unwrap(), value.to_string()).unwrap();
        let error = prepare(
            root.path(),
            tempfile::tempdir().unwrap().path(),
            &Config::load(root.path()).unwrap(),
            None,
            None,
        )
        .err()
        .expect("Missing immutable source must fail");
        assert!(error.to_string().contains("source"));
    }
    #[test]
    fn retained_translation_tree_uses_immutable_canonical_images() {
        let root = fixture("");
        write(
            root.path(),
            "docs/screenshots/tracking.png",
            "immutable-png",
        )
        .unwrap();
        write(
            root.path(),
            "docs/es/demos.md",
            "# Demos\n\n![Tracking](../screenshots/tracking.png)\n",
        )
        .unwrap();
        write(root.path(), "docs/release.json", "{\"immutable\":true}").unwrap();
        let english = "# Demos\n\n![Tracking](../screenshots/tracking.png)\n";
        write(
            root.path(),
            "docs/es/translations.json",
            json!({"demos.md":format!("{:x}",Sha256::digest(english.as_bytes()))}).to_string(),
        )
        .unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(root.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success());
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "commit",
            "-m",
            "docs",
        ]);
        let documentation = git(&["rev-parse", "HEAD"]);
        let translation = git(&["rev-parse", "HEAD:docs/es"]);
        let localized = localized(
            root.path(),
            Some(&documentation),
            Some(&translation),
            "docs/agents/demos.md",
            "es",
            english,
            &Settings::default(),
        )
        .unwrap();
        assert_eq!(localized.source_path, "docs/es/demos.md");
        assert!(localized.translation_subtree);
        let stage = tempfile::tempdir().unwrap();
        let pages = BTreeMap::from([("docs/es/quickstart.md".into(), "quickstart".into())]);
        let rewriter = Rewriter {
            root: root.path(),
            stage: stage.path(),
            reference: Some(&translation),
            documentation_ref: Some(&documentation),
            source_ref: &documentation,
            source_path: &localized.source_path,
            canonical_source_path: "docs/agents/demos.md",
            translation_subtree: localized.translation_subtree,
            scope: "/es/api/1.0.0/",
            prefix: "/",
            pages: &pages,
            repository: "example/library",
        };
        let output = rewriter
            .rewrite("![Tracking](../screenshots/tracking.png)", false)
            .unwrap();
        assert!(output.contains("/es/api/1.0.0/media/docs/screenshots/tracking.png"));
        assert!(rewriter
            .rewrite("[Release](../release.json)", false)
            .unwrap()
            .contains(&format!("/blob/{documentation}/docs/release.json")));
        assert!(rewriter
            .rewrite("[Sibling](quickstart.md)", false)
            .unwrap()
            .contains("[Sibling](/es/api/1.0.0/quickstart/)"));
        assert!(rewriter
            .rewrite("[Sibling](quickstart.md)", true)
            .unwrap()
            .contains("[Sibling](/es/api/1.0.0/raw/quickstart.md)"));
        assert_eq!(
            fs::read(
                stage
                    .path()
                    .join("static/es/api/1.0.0/media/docs/screenshots/tracking.png")
            )
            .unwrap(),
            b"immutable-png"
        );
    }
    #[test]
    fn untranslated_sibling_guide_uses_known_canonical_page_fallback() {
        let root = fixture("");
        let stage = tempfile::tempdir().unwrap();
        let pages = BTreeMap::from([("docs/agents/api.md".into(), "api".into())]);
        let rewriter = Rewriter {
            root: root.path(),
            stage: stage.path(),
            reference: None,
            documentation_ref: None,
            source_ref: "working-tree",
            source_path: "docs/es/agents/index.md",
            canonical_source_path: "docs/agents/index.md",
            translation_subtree: false,
            scope: "/es/api/development/",
            prefix: "/",
            pages: &pages,
            repository: "example/library",
        };
        assert!(rewriter
            .rewrite("[API](api.md)", false)
            .unwrap()
            .contains("[API](/es/api/development/api/)"));
    }
}
