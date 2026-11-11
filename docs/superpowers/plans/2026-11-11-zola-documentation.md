# Zola documentation implementation plan

Goal: deliver Demo Common's bilingual, versioned Zola site and reusable CLI integration.
Spec: ../specs/2026-11-11-zola-documentation-design.md

- [x] Rust backend: add sites/zola.toml dispatch, immutable content staging, translation freshness, scoped routes/raw Markdown/provenance, compatibility URLs, version manifest, and local output validation. Write and run real Zola regression tests before implementation; retain legacy renderer tests.
- [x] Shared theme: bundle templates and static assets for readable navigation, responsive layout, localized UI, scoped search, version/language selection, theme preference, screenshots, and raw/source links. Verify template rendering against the staged contract.
- [x] CI: generated workflows install checksum-pinned Zola when opted in; preserve custom Astro commands and Node setup for existing consumers. Run workflow regression tests.
- [ ] Demo Common: opt in, correct current English/Spanish setup/quickstart information, review translations and record source hashes, document builds and preview, regenerate workflows with the exact new tooling commit.
- [ ] Verify: all Rust tests, Demo Common full build/check without Python/Node, old/new links and archived content, desktop/mobile/light/dark/browser interactions. Review both diffs and publish linked pull requests. Keep merge/publishing status explicit.

Interfaces: staged scopes.json and public versions.json arrays contain language,module,label,version,root,source,documentation_ref,development,pages[{slug,title,href,raw}],images[{src,alt}]. Article extra fields scope_index,guide,fallback; templates can use config.extra.repository. Canonical source docs stay in docs/agents and docs/es. Theme assets are embedded in the CLI; repository sites/templates and sites/static may override them.

Verification checkpoint: 60 Rust tests pass, including six real-Zola tests; browser language/version/search/light-dark/mobile navigation passed. Three review findings were reproduced and fixed with regression tests: nested filename collision, linked-image rewriting and missing explicit translation tree.
