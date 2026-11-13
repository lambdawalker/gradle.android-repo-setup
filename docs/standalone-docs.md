# Standalone documentation

`android-repo-docs` maintains documentation throughout a project's lifetime. It is a separate executable and Cargo package in this repository. Installing it does not install `android-repo-setup`, and documentation builds do not need setup credentials or a configured publishing wizard.

Both tools share the import-guide, archive, and site implementations. Existing setup commands remain compatible. Setup retains responsibility for release reservations, artifact verification, and recording a confirmed publication. Documentation commands cannot confirm a publication.

## Install and initialize

Install a release containing the docs package, or an immutable source revision:

```sh
cargo install --locked --git https://github.com/lambdawalker/gradle.android-repo-setup --rev FULL_COMMIT_SHA android-repo-docs
android-repo-docs init --tooling-ref FULL_COMMIT_SHA
android-repo-docs bootstrap generate
```

The `--tooling-ref` pin must identify a release or commit containing `android-repo-docs`. The docs and setup pins are independent. A setup tag from before the docs executable was introduced cannot install the docs executable.

Commit `.project/docs/config.yaml`, `docs.sh`, `docs.bat`, and `.project/docs/bootstrap.ps1`. On Linux/macOS use `sh ./docs.sh`; on Windows use `docs.bat`. The launchers install the pinned executable into an ignored project cache, verify release checksums or build the exact pinned source, and make the executable available to child commands for the duration of the invocation. They do not modify the caller's PATH.

## Daily use

```sh
sh ./docs.sh check
sh ./docs.sh update
sh ./docs.sh build --check
sh ./docs.sh serve
```

`check` verifies configured generated documentation and reviewed inputs without updating them. `update` is an intentional regeneration step whose resulting diff should be reviewed. Media and screenshot acceptance remain separate explicit operations. `build` produces a local website; `serve` provides a loopback preview. Neither command deploys the site or publishes artifacts.

Use `imports --check` and `history verify` for focused installation/history validation. `history seed` is an explicit legacy migration operation. Release records and retained source archives remain authoritative; documentation generation never infers a release from development coordinates.

## Repository configuration

Independent metadata lives in `.project/docs/config.yaml`:

```yaml
schema: 1
repository: owner/android-library
title: Android library
modules:
  library: Library
tooling_ref: FULL_COMMIT_SHA
```

The tools accept a stable `vX.Y.Z` tag or a full lowercase commit SHA. Keep `tooling_ref` on one top-level line; a plain or quoted value with a trailing comment is supported. Existing setup metadata is a compatibility fallback, but an invalid docs config is always an error.

Source groups, example markers, and media scenarios belong in configuration. Project prose belongs in Markdown. See [source documentation](source-docs.md) and [documentation media](documentation-media.md) for those schemas.

## Shared Astro default and compatibility

New sites use bundled **Astro 7.3.5 + Starlight 0.42.4**. The installed CLI embeds the frontend package and lockfile; a separate npm package publication is unnecessary. Install Node.js 24 with npm (minimum Node 22.12), then use `sh ./docs.sh build --check`. The CLI prepares immutable content in a temporary app, installs locked dependencies with `npm ci`, renders and validates a temporary output, and replaces `sites/dist/` only after success. A failed build preserves the last good output. npm package downloads require registry access unless npm already has the needed packages cached.

```yaml
site:
  backend: astro
  base_url: https://owner.github.io/android-library/
```

The optional `base_url` fixes the public URL; otherwise an existing `sites/zola.toml` base URL is retained, or the GitHub Pages project URL is inferred from `repository`. `--base /preview/` overrides the path for a local preview. Both `build` and loopback `serve` use the same selected backend. Consumer guides remain plain Markdown; historical build code and MDX are never executed. The shared content pipeline preserves scoped raw guides, release/source/documentation provenance, English/Spanish fallback, assets and release catalogs.

| Configuration | Selected backend |
| --- | --- |
| `site.backend: astro` | Shared Astro, even if old Zola files still exist |
| `site.backend: zola` | Shared Zola, requires `sites/zola.toml` and Zola 0.23.6 |
| `site.backend: legacy` | Original minimal static renderer |
| Existing `site.build` / `site.serve` arrays | External commands |
| No explicit backend, existing `sites/zola.toml` | Zola compatibility |
| No backend, no external commands, no Zola file | Shared Astro default |

A built-in backend cannot also define external command arrays. Unknown backend values fail validation. `init` records the default in a new config and infers one module from the repository name if no module inventory exists; it never rewrites an existing config. Add canonical guides under `docs/agents/`, a `docs/releases.md` runbook and installation guidance before building. Adopting a new CLI release alone intentionally leaves existing Zola and external sites working; actual library migrations are separate changes. See [the consumer audit](astro-migration-audit.md) for requirements before switching each library.

The frontend and theme ship in `cli/docs/astro-theme/`. Trusted current-checkout overrides may be placed in `sites/astro/components/`, `sites/astro/styles/` and `sites/astro/public/`, mapped to the bundled application's `src/components/`, `src/styles/` and `public/`. Replacing the provided component/style filenames customizes the shared layout; unused new files are not automatically imported. Canonical prepared metadata and immutable assets take precedence over public overrides. Tera templates in `sites/templates/` are Zola-specific and must be ported explicitly.

Additional source pages, reviewed translations, and legacy routes can be configured without a custom renderer:

```yaml
site:
  backend: astro
  pages:
    README.md: project/overview
    docs/operations.md: operations
  current_pages:
    docs/maintenance.md: project/development
  translation_manifest: sites/translations.json
  translation_paths:
    README.md: docs/es/README.md
  aliases:
    - source: README.md
      html: '{language}/main/README.html'
      raw: 'raw/{language}/main/README.md'
```

`pages` adds canonical repository Markdown files or changes their guide slugs. It retains the discovered `docs/agents/` pages, `IMPORT.md`, and `docs/releases.md`; map a default source explicitly to change its slug. An empty slug selects the scope overview. Sources and slugs must be safe relative paths, and every guide route must be unique. Historical pages come from their immutable `documentation_ref`. `current_pages` uses the current working-tree source, translation and assets for every selected scope instead. Both its human page and raw Markdown identify it as a current shared project resource, and page metadata records the current source/documentation revision. It does not change publication records.

The translation manifest can contain simple SHA-256 values keyed by guide-relative or canonical source paths, or reviewed records such as `{"README.md":{"path":"docs/es/README.md","source_sha256":"…","provenance":"Reviewed Spanish prose"}}`. The source hash must match normalized canonical Markdown. Missing or stale translations fall back to the same source revision; fresh translations with changed fenced code fail the build. `translation_paths` takes precedence over a record's `path`. Relative links and images resolve from the actual translated file location; canonical and mapped translated page paths lead to the same scoped guide. Existing default translation locations remain supported.

Each alias names a configured or discovered canonical source. `html` writes a redirect to its prepared human page; `raw` copies its exact prepared raw Markdown, including provenance. Output paths expand `{language}`, `{module}`, and `{version}`. Paths are relative to the site root, must be unique after expansion, and cannot replace canonical pages, raw guides, metadata or generated assets. Include `{version}` when aliases apply to several immutable versions, or use a development-only project without historical scopes. Repository traversal and unknown source pages fail validation.

Current `sites/static/` assets are copied into the shared public directory for compatibility. `sites/astro/public/` can override them, followed by prepared canonical raw guides, metadata and immutable assets. No historical build script is executed. The temporary Astro app and transactional output both live under `sites/`, keeping Vite's asset renames on the destination filesystem. Root `agents/*.md` and `IMPORT.md` remain current Development inputs; historical raw guides retain their own scoped paths. Source-file links prefer the immutable publication source when the file exists there, and use `documentation_ref` for examples added by a documentation correction.

A project that needs a small domain-specific content projection may set `site.prepare: [node, sites/prepare.mjs]` with the built-in Astro backend. The command runs from the trusted current project checkout after content staging and public asset copies, before dependency installation/build. Arguments remain literal; `ANDROID_REPO_DOCS_STAGE` points to the temporary app containing `pages.json`, `scopes.json`, `site.json`, and `public/`. Immutable release catalog entries are available as each released scope's `release` object; `destinations` remains the compatible list of provider names. The hook may project these prepared files without rewriting source archives. A nonzero exit or recursive prepare invocation fails the build and preserves `sites/dist/`. `ANDROID_REPO_DOCS_PREPARE_ACTIVE=1` marks the child invocation. This hook does not select the external backend and cannot be combined with `site.build`/`site.serve` or a different built-in backend.

Existing project site designs can continue using literal command arrays:

```yaml
site:
  build: [npm, --prefix, sites, run, check]
  serve: [npm, --prefix, sites, run, dev]
```

Commands run from the project root, inherit the environment, and must return a successful exit status. On Windows, a missing bare executable also searches PATH for a `.cmd` or `.bat` shim, so the same `npm` arrays work with a standard Node installation. Arguments remain separate and use Rust’s batch-command quoting. Optional base URL, preview port, and check mode are exposed as `ANDROID_REPO_DOCS_BASE`, `ANDROID_REPO_DOCS_PORT`, and `ANDROID_REPO_DOCS_CHECK`. A custom build may call `android-repo-docs check`; recursively invoking the site build itself is rejected.

## Setup integration and CI

For a repository already using setup, creating the docs config opts it into shared bootstrap and workflow generation:

```sh
sh ./setup.sh bootstrap generate --overwrite
sh ./setup.sh workflows
```

The setup generator creates the docs launchers from the docs pin and preserves separate pins. Generated documentation jobs install the docs package directly. Publishing jobs install both executables when their validation needs both. The workflow updater accepts an independent docs pin and regenerates adopted launchers.

CI should install or expose an executable explicitly when a later step invokes it directly. Running a launcher does not add its cache to the parent shell or subsequent Actions steps. For example, a direct Gradle task that invokes `android-repo-setup` requires that executable on its own PATH.

Windows, Linux and macOS release artifacts are produced separately for each executable, with SHA-256 checksum files. Source-pinned launchers require Rust and Git. Astro jobs require Node/npm; explicit or detected Zola sites retain Zola 0.23.6. Android screenshot generation retains each project's own Gradle tasks and toolchain requirements.
