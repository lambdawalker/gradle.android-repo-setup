# Standalone documentation tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship independently usable android-repo-docs and migrate all existing Android setup CLI consumers.
**Architecture:** Separate library/binary package cli/docs in cli workspace. Setup uses the library for compatibility and confirmed-release transactions; docs has no dependency on setup. Declarative per-repository docs configuration and explicit check/update commands replace custom helpers.
**Tech Stack:** Rust, clap/serde, current Zola/Astro backends, syntax-aware Kotlin analysis, PNG decoding, generated POSIX/PowerShell launchers.
**Spec:** docs/superpowers/specs/2026-11-12-standalone-docs-design.md

## Global Constraints

- Keep confirmed release identity and historical content unchanged; archived scripts never execute.
- Docs binary does not require setup executable or signing/environment dependencies.
- Check commands never update reviewed source hashes or accepted images; update/accept is explicit.
- Repository names, classes, paths and prose belong in config/Markdown, not engine branches.
- Retain Zola 0.23.6, project site designs, existing Gradle plugin pins, Windows and POSIX support.

## Review Focus

- Cross-crate installation/history behavior parity and release transaction ownership.
- CRLF, non-ASCII, ambiguous/missing Markdown markers and syntax errors.
- Multi-module/custom roots, overloads, private nested declarations and added/deleted sources.
- Stale media provenance, duplicate scenarios and update validation before writes.
- Real docs-only installation, workflow PATH setup and tag/source-pin upgrade behavior.

### Task 1: Standalone package and existing docs extraction
**Files:** cli/Cargo.toml, cli/Cargo.lock, cli/src/docs.rs, cli/docs/{Cargo.toml,src/**,site-theme/**}, cli/site-theme/**.
**Interfaces:** android_repo_docs::{documentation,build,history,seed,archive_record,validate_archive,validate_record,HISTORY,local_identity}; maintain existing args/results; docs executable CLI check/update/imports/history/build/serve/init, source/media subcommands added subsequently.
- [ ] Add failing standalone executable parity tests for import docs/history/site operations.
- [ ] Extract implementation/theme into independent package, remove dependency on setup types; prefer `.project/docs/config.yaml` metadata and legacy setup fallback.
- [ ] Preserve setup compatibility via thin library re-exports and shared coordinate parsing. Implement site argv backend and local serve; check never publishes.
- [ ] Run full existing suite plus docs package tests, commit and review.

### Task 2: Configurable sources and examples
**Files:** cli/docs/src/sources/**, cli/docs/src/main.rs, cli/docs/tests/source_docs.rs, docs/source-docs.md.
**Interfaces:** sources::{init(root),check(root),update(root)} returning anyhow::Result; source settings within schema-1 docs config. Metadata parsed independently from sources settings. Produce an example config and exact schema documentation for consumer migration.
- [ ] Test generic fixtures: Kotlin declarations/overloads/visibility/defaults, scoped source roots, full-source hashes incl add/delete, full/region snippets, distinct Markdown markers, CRLF/idempotence, malformed input and traversal rejection.
- [ ] Implement syntax-aware source excerpts with deterministic outputs, configurable groups/intro files, whole-file or marked-section generation. Source manifest preserves project-specified path.
- [ ] Wire sources check/update/init into standalone aggregate commands without requiring publishing metadata for source-only projects.
- [ ] Test suite, commit, and review.

### Task 3: Reusable documentation media
**Files:** cli/docs/src/media/**, cli/docs/src/main.rs, cli/docs/tests/media.rs, docs/documentation-media.md.
**Interfaces:** config-driven media {verify,render,compare,accept}; screenshots {manifest,render,check,update}; generic paths/source commands/scenarios and manifests documented. Use actual Card Detector media.py and Permissions screenshots.mjs as behavior references only.
- [ ] Test frame hash/provenance, explicit acceptance, PNG decode/pixel comparison, missing/duplicate cases, stale captures, input add/delete, legacy copies, prevalidation before writes.
- [ ] Implement config-driven frame/screenshot features; preserve existing accepted assets and metadata formats during migration. Move existing setup screenshot-manifest function to docs library with compatibility delegation.
- [ ] Wire aggregate check and add generic tests, commit, review.

### Task 4: Bootstrap, workflows and release packaging
**Files:** cli/src/bootstrap.rs, cli/bootstrap/**, cli/src/workflows.rs, cli/templates/**, .github/workflows/**, cli/tests/**, docs/standalone-docs.md.
**Interfaces:** setup bootstrap generates docs.sh/docs.bat/.project/docs/bootstrap.ps1 only for opted-in docs config. docs tooling_ref independently pinned; updater regenerates/stages adopted docs bootstrap. Workflows install both packages where publishing and docs both needed; docs-only job can install docs alone.
- [ ] Test generated docs launchers with fake installations for args/project/exit, checksum/version failure, CRLF; workflow opt-in/pin behavior; release packaging both executables.
- [ ] Implement installation, workflow and independent binary packaging integration; preserve legacy consumers.
- [ ] Run workspace tests/format, commit, review, push tooling PR with exact SHA for consumers.

### Task 5: All current consumer migrations and end-to-end checks
**Files:** carddetector-shared/, demo-common/, permissions/, docs-auth/, docs-yolo/, docs-math.coordinates/ on task branches.
- [ ] Configure source/media mappings and independent docs metadata; move Card introductory prose to Markdown; replace helpers with direct tool calls.
- [ ] Update all documentation/site/release/CI references, preserve curated prose and images. Generate launchers/workflows with actual new CLI and immutable pushed pin.
- [ ] Run source checks, import/history checks, builds, site/link tests, relevant screenshot tests and bootstrap/workflow checks; inspect generated documentation diffs.
- [ ] Independent review, fix findings, push existing consumer PRs and refresh descriptions. Inspect CI; report actual remaining limitations.
