use super::{history, read, string, Config};
use anyhow::{ensure, Context, Result};
use pulldown_cmark::{html, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use regex::{Captures, Regex};
use std::{collections::BTreeSet, fs, io, path::Path, process::Command};
use unicode_normalization::UnicodeNormalization;

const GUIDES: &[&str] = &["index", "quickstart", "api"];
const ENGLISH: &[(&str, &str)] = &[
    ("index", "Overview"),
    ("quickstart", "Quickstart"),
    ("api", "API"),
    ("install", "Installation"),
    ("releases", "Publishing"),
];
const SPANISH: &[(&str, &str)] = &[
    ("index", "Resumen"),
    ("quickstart", "Guía rápida"),
    ("api", "API"),
    ("install", "Instalación"),
    ("releases", "Publicación"),
];

struct Version<'a> {
    name: String,
    reference: Option<&'a str>,
    entry: Option<&'a serde_json::Value>,
}

fn git_blob(root: &Path, reference: &str, path: &str) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["show", &format!("{reference}:{path}")])
        .output()?;
    ensure!(
        output.status.success(),
        "Could not read {path} at {reference}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

fn source(root: &Path, reference: Option<&str>, path: &str) -> Result<String> {
    if let Some(reference) = reference {
        let text = String::from_utf8(git_blob(root, reference, path)?)?;
        Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        read(root.join(path))
    }
}

fn localized_source(
    root: &Path,
    reference: Option<&str>,
    path: &str,
    lang: &str,
) -> Result<(String, bool)> {
    if lang == "es" {
        let name = path.rsplit('/').next().unwrap();
        let translated = format!("docs/es/{name}");
        let available = if let Some(reference) = reference {
            Command::new("git")
                .current_dir(root)
                .args(["cat-file", "-e", &format!("{reference}:{translated}")])
                .output()?
                .status
                .success()
        } else {
            root.join(&translated).is_file()
        };
        if available {
            return Ok((source(root, reference, &translated)?, false));
        }
    }
    Ok((source(root, reference, path)?, lang == "es"))
}

fn write(destination: &Path, path: &str, body: impl AsRef<[u8]>) -> Result<()> {
    let path = destination.join(path);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, body)?;
    Ok(())
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn layout(lang: &str, title: &str, nav: &str, body: &str, base: &str, config: &Config) -> String {
    let escaped = escape(title);
    let title = &config.title;
    format!(
        concat!(
            "<!doctype html><html lang=\"{}\"><meta charset=\"utf-8\">",
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
            "<title>{} · {}</title>",
            "<link rel=\"stylesheet\" href=\"{}/style.css\">",
            "<header><a href=\"{}/\">{}</a><nav>{}</nav></header>",
            "<main>{}</main></html>\n"
        ),
        lang, escaped, title, base, base, title, nav, body
    )
}

struct GuideLinks {
    fences: Regex,
    links: Regex,
    github: String,
}

impl GuideLinks {
    fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            fences: Regex::new(r"(?s)```.*?```")?,
            links: Regex::new(r"\]\(([^)]+)\)")?,
            github: format!("https://github.com/{}", config.repository),
        })
    }

    fn rewrite(&self, text: &str, base: &str, reference: Option<&str>, raw: bool) -> String {
        let rewrite_part = |text: &str| {
            self.links
                .replace_all(text, |capture: &Captures<'_>| {
                    let target = &capture[1];
                    if target.starts_with('#')
                        || target.starts_with("http://")
                        || target.starts_with("https://")
                    {
                        return capture[0].to_owned();
                    }
                    if target.ends_with("IMPORT.md") {
                        let page = if raw { "IMPORT.md" } else { "install.html" };
                        return format!("]({base}/{page})");
                    }
                    let page = target.split('#').next().unwrap();
                    if GUIDES
                        .iter()
                        .any(|guide| page.strip_suffix(".md") == Some(guide))
                    {
                        return if raw {
                            capture[0].to_owned()
                        } else {
                            format!("]({})", target.replacen(".md", ".html", 1))
                        };
                    }
                    let github = &self.github;
                    let reference = reference.unwrap_or("main");
                    let target = target.strip_prefix("../../").unwrap_or(target);
                    format!("]({github}/blob/{reference}/{target})")
                })
                .into_owned()
        };
        let mut rewritten = String::new();
        let mut cursor = 0;
        for fence in self.fences.find_iter(text) {
            rewritten.push_str(&rewrite_part(&text[cursor..fence.start()]));
            rewritten.push_str(fence.as_str());
            cursor = fence.end();
        }
        rewritten.push_str(&rewrite_part(&text[cursor..]));
        rewritten
    }
}

fn render(text: &str) -> String {
    let separator = Regex::new(r"[-\s]+").unwrap();
    let numbered_suffix = Regex::new(r"_(\d+)\z").unwrap();
    let table_tag = Regex::new(r"</?(?:table|thead|tbody|tr)>|</t[dh]>").unwrap();
    let mut used = BTreeSet::new();
    let mut parser = Parser::new_ext(text, Options::ENABLE_TABLES);
    let mut events = Vec::new();
    while let Some(event) = parser.next() {
        if let Event::Start(Tag::Heading { level, .. }) = event {
            let mut contents = Vec::new();
            let mut heading = String::new();
            for inner in parser.by_ref() {
                match &inner {
                    Event::Text(text) | Event::Code(text) => heading.push_str(text),
                    Event::SoftBreak | Event::HardBreak => heading.push(' '),
                    _ => {}
                }
                let end = matches!(inner, Event::End(TagEnd::Heading(_)));
                contents.push(inner);
                if end {
                    break;
                }
            }
            let ascii: String = heading
                .nfkd()
                .filter(char::is_ascii)
                .filter(|c| {
                    c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || c.is_ascii_whitespace()
                })
                .collect();
            let mut id = separator
                .replace_all(ascii.trim(), "-")
                .to_ascii_lowercase();
            while id.is_empty() || used.contains(&id) {
                id = if let Some(capture) = numbered_suffix.captures(&id) {
                    let suffix = capture.get(0).unwrap();
                    let next = capture[1]
                        .parse::<u64>()
                        .ok()
                        .and_then(|value| value.checked_add(1));
                    match next {
                        Some(next) => format!("{}_{next}", &id[..suffix.start()]),
                        None => format!("{id}_1"),
                    }
                } else {
                    format!("{id}_1")
                };
            }
            used.insert(id.clone());
            events.push(Event::Start(Tag::Heading {
                level,
                id: Some(id.into()),
                classes: Vec::new(),
                attrs: Vec::new(),
            }));
            events.extend(contents);
        } else if let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_))) = &event {
            events.push(event);
            for inner in parser.by_ref() {
                match inner {
                    Event::Text(text) => events.push(Event::Html(
                        text.replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;")
                            .replace('"', "&quot;")
                            .into(),
                    )),
                    Event::End(TagEnd::CodeBlock) => {
                        events.push(inner);
                        break;
                    }
                    _ => events.push(inner),
                }
            }
        } else if let Event::Start(Tag::Table(_)) = &event {
            let mut table = vec![event];
            for inner in parser.by_ref() {
                let end = matches!(inner, Event::End(TagEnd::Table));
                table.push(inner);
                if end {
                    break;
                }
            }
            let mut markup = String::new();
            html::push_html(&mut markup, table.into_iter());
            let formatted = table_tag.replace_all(&markup, "$0\n").replace("\n\n", "\n");
            events.push(Event::Html(formatted.into()));
        } else {
            events.push(event);
        }
    }
    let mut output = String::new();
    html::push_html(&mut output, events.into_iter());
    output.trim_end_matches('\n').to_owned()
}

pub(crate) fn build(root: &Path, base: Option<&str>) -> Result<()> {
    let config = Config::load(root)?;
    let default_base = format!("/{}", config.repository.split('/').next_back().unwrap());
    let base = base.unwrap_or(&default_base).trim_end_matches('/');
    let destination = root.join("sites/dist");
    match fs::remove_dir_all(&destination) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Could not remove the previous docs build"),
    }
    fs::create_dir_all(&destination)?;
    fs::copy(root.join("sites/style.css"), destination.join("style.css"))?;
    let entries = history::catalog(root, &config)?;
    history::verify(root, &config, &entries)?;
    let mut versions = vec![Version {
        name: "development".into(),
        reference: None,
        entry: None,
    }];
    for entry in &entries {
        versions.push(Version {
            name: history::route_version(entry)?,
            reference: Some(string(entry, "documentation_ref")?),
            entry: Some(entry),
        });
    }
    let latest = &versions.last().unwrap().name;
    let links = GuideLinks::new(&config)?;
    for version in &versions {
        let name = &version.name;
        let reference = version.reference;
        for (lang, other, labels) in [("en", "es", ENGLISH), ("es", "en", SPANISH)] {
            let folder = format!("{name}/{lang}");
            let url = format!("{base}/{folder}");
            let install = if let Some(entry) = version.entry {
                history::installation(entry, lang)?
            } else {
                source(root, None, "IMPORT.md")?
            };
            write(&destination, &format!("{folder}/IMPORT.md"), &install)?;
            for &(page, title) in labels {
                let version_menu = versions
                    .iter()
                    .map(|version| {
                        let name = &version.name;
                        format!(
                            "<a href=\"{base}/{name}/{lang}/{page}.html\">{}</a>",
                            escape(name)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut nav = labels
                    .iter()
                    .map(|(page, label)| format!("<a href=\"{url}/{page}.html\">{label}</a>"))
                    .collect::<Vec<_>>()
                    .join(" ");
                nav.push_str(&format!(
                    " <a href=\"../{other}/{page}.html\">{}</a>",
                    other.to_ascii_uppercase()
                ));
                let guide = GUIDES.contains(&page);
                let (text, fallback) = if page == "install" {
                    (install.clone(), lang == "es" && version.entry.is_none())
                } else {
                    let path = if guide {
                        format!("docs/agents/{page}.md")
                    } else {
                        "docs/releases.md".into()
                    };
                    localized_source(root, reference, &path, lang)?
                };
                let mut body = format!("<p>Version {} · {version_menu}</p>", escape(name));
                if fallback {
                    body.push_str("<p lang=\"es\">Guía en inglés para esta versión.</p>");
                }
                body.push_str(&render(&links.rewrite(&text, &url, reference, false)));
                if page == "index" {
                    body.push_str(&format!("<div class=\"gallery\"><img src=\"{base}/media/{name}/catalog-light.png\" alt=\"Populated light catalog\"><img src=\"{base}/media/{name}/catalog-dark.png\" alt=\"Populated dark catalog\"></div>"));
                }
                let rendered = layout(lang, title, &nav, &body, base, &config);
                write(&destination, &format!("{folder}/{page}.html"), rendered)?;
                if guide {
                    write(
                        &destination,
                        &format!("{folder}/agents/{page}.md"),
                        links.rewrite(&text, &url, reference, true),
                    )?;
                }
            }
        }
    }
    let title = &config.title;
    write(&destination, "index.html", layout("en", "Documentation", "", &format!(
        "<meta http-equiv=\"refresh\" content=\"0;url={base}/{latest}/en/index.html\"><a href=\"{base}/{latest}/en/index.html\">Open {title} documentation</a>"
    ), base, &config))?;
    fs::copy(
        destination.join(format!("{latest}/en/IMPORT.md")),
        destination.join("IMPORT.md"),
    )?;
    fs::create_dir(destination.join("agents"))?;
    for guide in GUIDES {
        let filename = format!("{guide}.md");
        fs::copy(
            destination.join(format!("{latest}/en/agents/{filename}")),
            destination.join("agents").join(filename),
        )?;
    }
    write(&destination, "llms.txt", format!("# {title}\n\n- [Integration guide]({base}/agents/index.md)\n- [Installation]({base}/IMPORT.md)\n"))?;
    for version in &versions {
        let name = &version.name;
        for filename in ["catalog-light.png", "catalog-dark.png"] {
            let path = format!("docs/screenshots/{filename}");
            let image = if let Some(reference) = version.reference {
                git_blob(root, reference, &path)?
            } else {
                fs::read(root.join(path))?
            };
            write(&destination, &format!("media/{name}/{filename}"), image)?;
        }
    }
    Ok(())
}
