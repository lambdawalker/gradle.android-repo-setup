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

The built-in site renderer supports the shared Zola theme. Existing project site designs can use literal command arrays:

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

Windows, Linux and macOS release artifacts are produced separately for each executable, with SHA-256 checksum files. Source-pinned launchers require Rust and Git. The built-in site backend retains Zola 0.23.6; Android screenshot generation retains each project's own Gradle tasks and toolchain requirements.
