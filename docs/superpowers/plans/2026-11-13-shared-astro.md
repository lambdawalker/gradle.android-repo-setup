# Shared Astro Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make bundled Astro/Starlight the new documentation default while preserving existing builders until consumer migration.
**Architecture:** Reuse Rust content staging for both built-in renderers. Bundle locked frontend sources into the installed executable and select backends consistently in the docs CLI and setup workflow generator.
**Tech Stack:** Rust, Astro 7.3.5, Starlight 0.42.4, Node 24, npm.
**Spec:** docs/superpowers/specs/2026-11-13-shared-astro-design.md

## Global Constraints
- Do not modify or deploy consumer libraries in this change; maintainer publishes CLI first.
- Preserve confirmed release records, immutable source identity, translations, raw guides and existing backend behavior.
- Build historical Markdown as data; never execute archived build scripts or MDX.

## Review Focus
- Existing external commands and Zola detection remain compatible.
- Root and GitHub Pages base paths preserve assets and scoped links.
- Missing/stale translations retain visible fallback at the original revision.
- A cargo-installed CLI carries all frontend sources and locked dependencies.
- Failed site builds cannot silently publish partial output or modify canonical sources.

### Task 1: Audit consumers
- [x] Read main configurations/workflows/renderers for all seven identified library sites.
- [x] Write docs/astro-migration-audit.md with current evidence, migration work and order.

### Task 2: Backend selection and workflow generation
Files: cli/docs/src/backend.rs, config.rs, lib.rs; cli/src/workflows.rs; backend/workflow tests.
- [x] Add failing backend precedence/conflict/default tests and an explicit Astro-on-Zola workflow test.
- [x] Implement explicit backend selection, Astro default, external compatibility and shared runtime detection.
- [x] Verify tests and default initialization; document schema.

### Task 3: Shared immutable content and Rust Astro adapter
Files: cli/docs/src/content.rs, zola.rs, astro.rs; cli/docs/tests/astro.rs.
- [x] Add manifest/history/base-path tests, then extract existing immutable staging without duplicating it.
- [x] Emit renderer-neutral page/scope/site JSON and unchanged Zola files.
- [x] Embed frontend assets; install locked dependencies; invoke build with useful runtime diagnostics and portable npm launching.
- [x] Verify existing Zola and new Astro tests and read-only inputs.

### Task 4: Bundled frontend
Files: cli/docs/astro-theme/**.
- [x] Add fixture/navigation/heading tests before implementation.
- [x] Build shared Starlight pages, scope selectors, provenance/fallback and raw links from prepared JSON.
- [x] Verify real Astro builds at root and project prefixes with pinned lockfile.

### Task 5: Integrate and deliver
- [x] Run full Rust workspace suite and frontend tests/build; inspect generated workflows and cargo package contents.
- [x] Review content safety, compatibility, migration audit and release guidance.
- [x] Open a CLI PR with checks and limitations. Do not publish CLI or migrate consumers.
