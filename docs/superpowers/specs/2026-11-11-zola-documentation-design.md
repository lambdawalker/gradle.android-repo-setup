# Zola documentation backend

Use Demo Common as the first consumer of a shared Zola documentation theme and Rust content preparation pipeline. Preserve the confirmed release archive, separate development from released API, and retain public links. Other repositories keep their renderer until opting in with sites/zola.toml.

The shared CLI stages Markdown from immutable documentation revisions using current code, applies optional translation-tree corrections, checks English source hashes before selecting Spanish translations, and creates exact-provider installation pages. Zola 0.23.6 renders the prepared content. Browser JavaScript supplies search, menus and theme controls without Node/Python build dependencies.

The website presents a responsive sidebar, on-page contents, language/version controls that retain the selected guide, raw Markdown, provenance, screenshots, and accessible search scoped to language/module/version. Canonical scopes follow /<language>/<module>/<version>/, with /raw/*.md counterparts. Old generator URLs remain navigable. Generated HTML validation covers local links, anchors and images. Missing historical objects fail clearly. Historical scripts are never executed; canonical docs/history are never rewritten by a build.

Alternatives: extending the small built-in HTML renderer would duplicate a site framework; migrating Demo Common to Astro would contradict the requested Rust tooling direction. Adopt opt-in Zola with a bundled customizable theme. No new library publication is needed for website-only changes.
