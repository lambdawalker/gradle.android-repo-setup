# Android repository setup Gradle plugins

Module discovery and multi-library setup are available in the **0.1.11 source release**: [module selection, independent publishing, and migration guide](docs/module-discovery.md). Run `android-repo-setup` for a first menu of detected libraries/apps, or `android-repo-setup discover` to inspect them without saving.

Gradle builds and publishes Android artifacts. The companion `android-repo-setup` CLI configures the project, manages release reservations, and verifies public artifacts. The independent `android-repo-docs` executable maintains documentation, examples, reviewed media, and local sites. See the [standalone documentation guide](docs/standalone-docs.md).

## Use a published release

The current release is [v0.1.7](https://github.com/lambdawalker/gradle.android-repo-setup/releases/tag/v0.1.7). It includes the Rust CLI binaries and the Gradle plugins published through JitPack. The CLI and Gradle plugins are separate installations; a CLI download does not install a Gradle plugin.

Map the plugin IDs to the JitPack module in `settings.gradle.kts`:

```kotlin
pluginManagement {
    resolutionStrategy {
        eachPlugin {
            if (requested.id.id in setOf(
                    "com.apexfission.android-repo-setup.root",
                    "com.apexfission.android-repo-setup",
                )) {
                requested.version?.let { version ->
                    useModule("com.github.lambdawalker:gradle.android-repo-setup:$version")
                }
            }
        }
    }
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
        maven("https://jitpack.io")
    }
}
```

Then use the release tag in the root `build.gradle.kts`:

```kotlin
plugins {
    id("com.apexfission.android-repo-setup.root") version "v0.1.7"
    id("com.apexfission.android-repo-setup") version "v0.1.7" apply false
}
```

Apply the library plugin in each publishable Android module as shown below. [Demo Common](https://github.com/lambdawalker/android.apexfission.demo-common) is the reference integration, including setup launchers and a bilingual Zola documentation site.

## Try it from a source checkout

Check out this repository alongside the Android project and add the included build to the project's `settings.gradle.kts`:

```kotlin
pluginManagement {
    includeBuild("../gradle.android-repo-setup")
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
```

Apply `com.apexfission.android-repo-setup.root` in the root build to register `generateImportDocs` and `verifyImportDocs`. Both tasks invoke the installed `android-repo-setup` CLI.

In each publishable Android library module, apply the library plugin after AGP:

```kotlin
plugins {
    id("com.android.library")
    id("com.apexfission.android-repo-setup")
}
```

Remove the module's existing Vanniktech plugin declaration and its release configuration when migrating. Keep the Android configuration, dependencies, and project-specific `mavenPublishing { pom { ... } }` metadata. Set the `GROUP` Gradle property for Central and `JITPACK_GROUP` for JitPack, such as `com.github.lambdawalker`. The Central artifact ID defaults to the module name. Override it with `POM_ARTIFACT_ID.<module>` or the shared `POM_ARTIFACT_ID` property.

The library plugin configures the release AAR, sources and documentation jars, reproducible jar settings, and the local `verification` Maven repository. Existing workflows can continue to use `-PreleaseModule`, `-PreleaseVersion`, `-PjitpackBuild=true`, and `-PjitpackScopedTag=true`. Maven Central uploads require the configured credentials and a valid release reservation.

Runnable application modules can use the shared `com.apexfission.android-repo-setup.application` plugin from a source checkout, or from a published release that contains it. It centralizes generic APK signing inputs plus public runtime/endpoint bindings while leaving encrypted-vault access in the CLI. See [Android application conventions](docs/application-conventions.md).

Generic model and binary asset preparation is documented in [Release assets](docs/release-assets.md) (next CLI release).

## JitPack

Use a published plugin version or make the pinned source checkout available to the tagged build through `includeBuild`. Configure the JitPack install command for the selected module, for example:

```yaml
install:
  - ./gradlew --no-daemon :demo-common:publishJitPack
```

When JitPack sets `JITPACK=true`, the plugin reads `VERSION` and validates the canonical `module/vX.Y.Z-jitpack` tag against the checked-out commit. It also compares `GIT_COMMIT` when provided. The selected module publishes to Maven Local using the scoped JitPack coordinates. This entry point requires Gradle and Git, plus the project's Android build toolchain.

## Development

Use JDK 21 and run `./gradlew check` (`gradlew.bat check` on Windows). The wrapper uses Gradle 9.6.0. Demo Common is the reference for publication behavior. Validate an Android project's release tasks in that project after applying the plugins.


## Publish to the Gradle Plugin Portal

1. Sign in to the [Gradle Plugin Portal](https://plugins.gradle.org/) and
   generate a publishing API key. Make sure your Portal account can publish
   both plugin IDs. Gradle may request a DNS TXT record to verify ownership
   of `apexfission.com` during the first submission.
2. In this repository, create a GitHub environment named
   `gradle-plugin-portal`. Add environment secrets `GRADLE_PUBLISH_KEY` and
   `GRADLE_PUBLISH_SECRET`. Restrict deployments to `main`.
3. Merge the release changes, confirm **Check Gradle plugins** passes, and run
   **Publish Gradle plugins** from `main`. The workflow publishes the version
   declared in `build.gradle.kts`. Confirm both plugin IDs are available on
   the Portal before migrating consumers.
4. For later releases, bump the version in `build.gradle.kts` before dispatch.
   If a submission is pending or its result is unclear, check the Portal
   before retrying the same version.

To check and build the publication locally with JDK 21, run
`./gradlew check publishToMavenLocal`. To run the Portal validation without
uploading, set the Portal credentials and run `./gradlew publishPlugins --validate-only`.
The Portal may review the first submission before it becomes available.

## Publish to JitPack

Use **Actions → Build Gradle plugins for JitPack** to test the Maven
publication locally and download it as a workflow artifact. Leave **source_tag**
blank to build `main`, or enter an existing project tag to build that commit.

Use **Actions → Publish Gradle plugins to JitPack** to build, trigger JitPack,
and verify the tagged commit and both plugin descriptors. For older tags,
run from `main` and enter the tag in **source_tag**. For tags that already
contain the current workflow, selecting the tag in **Use workflow from**
also works.

Use **Actions → Publish CLI and JitPack** to publish both in parallel. Run
it from `main` with **source_tag** blank to bump both components to the next
patch version above the current versions and release tags. The workflow
pushes the version commit and one shared `vX.Y.Z` tag together, then publishes
the CLI to GitHub Releases and the Gradle plugins to JitPack. The workflow
token needs permission to push to `main` and create tags.

To publish an existing tag or retry after a publishing failure, enter the
tag in **source_tag**. The workflow reuses the tagged source without another
version bump.
The original `v0.1.0` tag predates the single-artifact JitPack configuration.

Consumers using JitPack can add `https://jitpack.io` to `pluginManagement.repositories` and map both plugin IDs to `com.github.lambdawalker:gradle.android-repo-setup:vX.Y.Z` with `resolutionStrategy.eachPlugin` and `useModule`.

## Standalone setup CLI

Build the Rust CLI from the repository root:

```sh
cargo build --locked --release --manifest-path cli/Cargo.toml
```

Download prebuilt executables from [release v0.1.7](https://github.com/lambdawalker/gradle.android-repo-setup/releases/tag/v0.1.7). Each archive has a matching `.sha256` file; verify it before extracting and adding the executable to `PATH`.

| Platform | Archive |
| --- | --- |
| Windows x64 | `android-repo-setup-x86_64-pc-windows-msvc.zip` |
| Linux x64 | `android-repo-setup-x86_64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `android-repo-setup-aarch64-apple-darwin.tar.gz` |

For Linux x64:

```sh
release=https://github.com/lambdawalker/gradle.android-repo-setup/releases/download/v0.1.7
archive=android-repo-setup-x86_64-unknown-linux-gnu.tar.gz
curl --fail --location --remote-name "$release/$archive"
curl --fail --location --remote-name "$release/$archive.sha256"
sha256sum -c "$archive.sha256"
tar -xzf "$archive"
./android-repo-setup --version
# Expected: android-repo-setup 0.1.7
```

Prebuilt binaries do not require Cargo. Building from source does. APK and Maven signing commands separately require `keytool` and GnuPG. New shared documentation sites use bundled Astro/Starlight and require Node.js 24 with npm (minimum Node 22.12). Existing Zola sites require Zola 0.23.6 on `PATH`; the Zola executable is not bundled.

Demo Common's `setup.bat` reads `tooling_ref`, downloads and verifies the matching Windows release, and caches it under `bin/<tag>/`. Its `setup.sh` validates the matching CLI on `PATH`. Full source SHA pins use a cached Cargo build instead.

Use **Actions → Build Rust CLI** to create downloadable test builds. Use
**Actions → Publish Rust CLI** to build and publish release assets. Run it
from `main` and leave **source_tag** blank to create the CLI tag for the
version in `cli/Cargo.toml`. Enter an existing tag such as `v0.1.1` in
**source_tag** to build that commit and publish its CLI binaries under that
tag. The build uses the selected tag's version for the executable. If a
GitHub Release already exists for the tag, the workflow attaches the CLI
archives and checksums to it. Pushing a `cli-vX.Y.Z` tag also triggers
publication.

Run the resulting `android-repo-setup` executable from an Android checkout, or
pass `--project <path>` before the command. On Windows the binary has an
`.exe` extension. It reads the existing `.project/setup/config.yaml`.

Running `android-repo-setup` opens the [iocraft](https://github.com/ccbrown/iocraft) deployment wizard. It includes
APK signing, GitHub signing and publishing environments, workflow previews,
and vault viewing and recreation. The standalone `signing` and
`setup_github_environments` commands use the same interface.

### Public Android signing associations

Selecting **APK signing key** creates or verifies the keystore and writes a public
association file for App Links and passkey setup:

```text
~/.projects/<repo-name>/secrets/setup-<id>.vault.associations.json
```

It appends `.associations.json` to the vault's complete filename and sits beside
`setup-<id>.vault`. The ID follows the vault's repository/signing
environment identity; separate environments get separate files. `--state-dir`
and `--vault-path` overrides also apply to the export location. The filename ID
is unrelated to the certificate fingerprint contained in the file.

Enter the actual installed Android application IDs when prompted; the wizard
remembers them in the encrypted vault. To supply project defaults, add:

```yaml
signing:
  application_ids:
    - com.example.app
```

Configured IDs take precedence over remembered answers and are shown for review.
Use each variant's resolved `applicationId`, including any suffix; a Gradle
namespace or repository name might differ. List multiple IDs only when those apps
use this signing certificate. For example, do not list a debug application here
if it is signed by a separate debug keystore.

The export uses the existing backend import schema:

```json
[
  {
    "packageName": "com.example.app",
    "sha256CertFingerprints": ["<uppercase colon-separated SHA-256 fingerprint>"]
  }
]
```

The CLI hashes the selected alias's DER certificate, not the APK or keystore
file. The JSON contains only public package names and fingerprints; keys and
passwords stay private. Each successful run atomically replaces this generated
snapshot with the selected apps and current certificate, without accumulating
old keys. Failed extraction preserves the previous file.

To export an existing key, run `android-repo-setup signing`, select **APK signing
key**, and reuse the existing keystore and alias. A GitHub-only run that finds
all remote secrets already configured does not inspect those secrets or export
an identity; use the local key operation to refresh the file.

In Attestra's **Add from public JSON** action, enter the printed export path and
approve it for the intended backend environment. Attestra still owns publishing
`/.well-known/assetlinks.json`; this CLI does not deploy it. The signing
environment (default `demo-apk-release`) does not select a backend environment.
For Google Play builds, use the Play app-signing certificate when it differs
from the local/upload certificate.

### Wizard controls

Use Tab and Shift+Tab to move between controls, Space to toggle a checkbox,
and Enter to activate a button. Ctrl+V pastes a single line into the focused
field. Password fields are masked. Saved answers default to reuse after
20 seconds, and interacting with the screen pauses the countdown. Review
screens require checking the confirmation box before Apply becomes available.
The Maven key picker loads local GnuPG keys in the background and offers a
form for creating and verifying a new project key. Use a terminal of at least
80 columns by 24 rows. Page Up and Page Down scroll the current form or details;
hold Shift to scroll the progress log. The focused field or key remains visible
when navigating with the keyboard. Mouse clicks select controls, and the wheel
scrolls the panel under the pointer.

```sh
android-repo-setup workflows --check
android-repo-setup workflows
android-repo-setup workflows --only-missing
android-repo-setup workflows --overwrite
```

Check mode shows a diff and exits nonzero on drift without writing files.
The default command refuses to overwrite existing differences.
Use `--overwrite` after reviewing the diff. Use only one workflow generator
at a time while migrating from Python.

To migrate a project that used the Python package, install the CLI on `PATH`
and pin the corresponding Git tag or full commit SHA in the project config.
Existing Python release journal tags and encrypted setup vaults can be read
by the Rust CLI. Review and regenerate your project files:

```sh
android-repo-setup workflows --check
android-repo-setup workflows --overwrite --set-tooling-ref <rust-tag-or-commit>
android-repo-setup publishing_config generate
```

Generated GitHub workflows install the pinned Rust CLI through Cargo. The
Gradle plugin uses the same `android-repo-setup` command on `PATH`. If the
consumer still has a Python JitPack bootstrap, switch it to the Gradle
`publishJitPack` task above, or invoke `android-repo-setup jitpack_build` in an
image where the binary is installed.

Run `android-repo-setup` with no subcommand to open the setup menu. It offers
APK key generation, GitHub environment configuration, workflow updates, and
vault inspection and reset. APK key generation requires the JDK's `keytool`.
Maven signing key selection and generation use `gpg`. On Windows, install
Gpg4win for a native GnuPG keyring. Run
`android-repo-setup signing --help` for direct signing and vault options, or
`android-repo-setup --help` for the headless commands.

### Verify an unsigned local publication

After Gradle builds the verification publication, run the Rust verifier from
the same clean checkout:

```sh
android-repo-setup module_release check-local --module demo-common --version 9.8.7 --source "$(git rev-parse HEAD)"
```

The destination defaults to Maven Central. Set `RELEASE_REPOSITORY=jitpack`
for an unsigned JitPack candidate. The command checks the tracked checkout,
coordinates, POM, AAR, sources, documentation and Gradle metadata and prints
the candidate record with SHA-256 hashes. It does not write release tags or
upload artifacts. By default the artifact contract follows Demo Common, including
its exported Compose dependencies and Java 17 bytecode. Other libraries can
configure the contract in `.project/setup/config.yaml`:

```yaml
verification:
  class_prefix: com/apexfission/android/permissions/
  required_api_artifacts: [permissions]
  bytecode_major: 61
```

`class_prefix` is a slash-separated package prefix ending in `/`. The AAR must
contain classes under that prefix with the configured JVM class-file major
version. Every `required_api_artifacts` entry must be exported by the POM at
compile scope (an omitted scope also means compile). Each omitted field retains
its Demo Common default: `com/apexfission/android/democommon/`, `[foundation, ui]`,
and `61`. An explicit empty API artifact list is supported for libraries without
exported dependencies. All other publication checks still apply. Local checks
and public confirmation both load this contract from the reserved source commit,
including when confirmation runs from another checkout.

To confirm an existing annotated reservation against the public registry, run:

```sh
android-repo-setup module_release confirm --module demo-common --version 0.1.2
```

Use the same `RELEASE_REPOSITORY` as the reservation. The command uses the
current commit as the expected source by default. Pass `--source` with the full
commit SHA when running from another checkout of the same repository. Maven
Central confirmation compares artifact hashes and checks for detached signature
files. JitPack confirmation verifies the build's commit and records hashes of
the public artifacts. The command writes `build/confirmed-demo-common.json` for
Central or `build/jitpack/confirmed-demo-common.json` for JitPack.

If propagation times out, preserve the reservation and rerun confirmation.
`--timeout` controls the polling window in seconds and defaults to 2400.

The generated GitHub workflows serialize releases for each module. When
running release commands directly, keep one active release process per module
and destination, including when the processes target different versions. The
Maven Central upload guard renews its pending tag atomically when recording the
upload marker. Repository rules must permit updates to `release-pending/*`
for the release workflow identity.

### Reconcile failed JitPack attempts

Publishing now reconciles earlier attempts before selecting a version and after an
unsuccessful reserved upload. Verified failures are archived without blocking the
next version; fully verified publications finish their metadata without uploading
again. Processing, missing receipts, and uncertain outcomes remain pending. See
[release reconciliation](docs/release-reconciliation.md) for the state rules,
Central credentials, and recovery commands.

Use **Reconcile failed JitPack attempts**, generated by setup, to review stale
pending journals. Leave **apply** unchecked for a preview. Enter space-separated
versions to restrict the review, or leave the field blank to examine every
pending attempt for the configured library module. Review the run summary,
then run it again with **apply** checked. Run the workflow from `main`; it shares
the publication concurrency group and uses the repository's `GITHUB_TOKEN`.
After upgrading the CLI, regenerate and commit the workflows to add this action.

The equivalent local commands require a checkout with `origin/main` and Git
credentials that can create and delete release journal tags:

```sh
android-repo-setup module_release reconcile-failed-jitpack --module demo-common
android-repo-setup module_release reconcile-failed-jitpack --module demo-common --version 0.1.0 --version 0.1.1 --apply
```

Each attempt must have an annotated, valid pending journal and an unchanged
source tag. Historical module tags such as `demo-common/v0.1.0` retain their
original JitPack consumer version; reconciliation does not rename them or create
replacement canonical tags. JitPack must report `Error` or `failed` for that exact
source commit, both before and after all publication artifact URLs return 404.
Confirmed versions, upload markers, public artifacts, unavailable or ambiguous
status, mismatched sources, and concurrent journal changes are preserved and
reported as `needs-review`. Other eligible failures can still be archived.

Automatic preparation reconciles all previous attempts for the selected module
and destination before choosing a version. A newer confirmed release does not prevent archiving a proven older
failure, but confirmation of the attempted version in either current metadata
or historical records always prevents it. `tagNotFound` without a source commit
is uncertain, not proof of failure. Keep its pending journal. To intentionally
publish a separate release, enter a version newer than all module history and
enable `bypass_version_verification` in the publish action, or pass
`--version X.Y.Z --allow-pending-jitpack` to `module_release prepare`. The override
preserves prior attempts and continues to reject upload-started markers.

Applying archives the **original annotated Git object** under
`release-failed/jitpack/<module>/<version>` and deletes its pending reference in
one atomic push with expected-object leases. Canonical version tags and confirmed
release records remain unchanged; this does not delete a JitPack build or make a
failed version reusable. Rerunning skips attempts already archived. If any entry
needs review, investigate its summary and rerun after resolving the cause; never
manually discard a reservation merely because a workflow failed. Direct CLI
runs should not overlap other release operations for that module.

### Demo application module

Set `workflows.app_module` in `.project/setup/config.yaml` to the Gradle module
that builds the demo APK. For a conventional `app` module, use:

```yaml
workflows:
  module: demo-common
  app_module: app
  title: Demo Common
```

Workflow generation uses this module for debug assembly and lint, signed APK
assembly, and the APK output directory. Existing configurations default to
`sample`. Run `android-repo-setup workflows --overwrite` after renaming a module
and reviewing the generated workflow differences.

### Workflow customization

The workflow generator accepts these optional settings under `workflows`:

```yaml
workflows:
  module: permissions
  app_module: app
  title: Permissions
  screenshot_count: 14
  java_version: '25'
  documentation_command: npm --prefix sites ci && npm --prefix sites run check
  screenshot_check_command: node sites/scripts/screenshots.mjs manifest
  release_validation_command: bash scripts/validate-release-source.sh
```

The defaults are five screenshots, Java 21, `android-repo-setup build_docs`,
`cd docs/screenshots && sha256sum -c images.sha256`, and `true` for release
validation. Java 17 is also installed for projects that need it. Documentation
and publication jobs use the same backend selection as the documentation CLI: shared Astro and external sites receive Node 24; explicit or detected Zola sites receive checksum-pinned Zola 0.23.6. Commands preserve shell punctuation and
may contain multiple lines. Release validation runs after checking out the
immutable release source and before verifying the unsigned publication; it
receives `SOURCE_SHA` and is skipped when the release is already complete.
Validation scripts should leave tracked source files unchanged, for example by
running documentation generation in a disposable worktree.

Release fingerprints include `.project/setup/config.yaml` and
`.project/setup/repositories.yml`, so changing the verification contract or
repository configuration changes the release identity. Finalization also
requires `.project/setup` to agree with the current main branch.

### Shared version identity

New module versions use one immutable source and build-input fingerprint across
Maven Central and JitPack. The first destination reserves the annotated
`release-identity/<module>/<version>` tag atomically with its publication journal.
The tag records `schema`, `module`, `version`, `source`, and `fingerprint`;
publication records carry the same object as `release_identity`.

Publishing that version to another destination reuses its exact source commit.
Matching inputs reuse the canonical version; changed inputs require a newer
version. Explicit versions cannot replace a reserved identity, and registry
verification overrides do not bypass this rule. The fingerprint covers tracked
library sources, build configuration, toolchain pins, publishing helpers and
verification settings. This guarantees matching source/build inputs, rather
than identical provider artifact bytes: signing and publication metadata can
differ.

Destinations retain separate pending/upload markers, hashes and confirmation
status. If publication is interrupted, use the existing finalization flow for
the same destination and version; an uncertain Central upload must never be
blindly uploaded again. Failed attempts retain their immutable shared identity.
Confirmed destinations appear together in the version's documentation archive.

Historical provider-specific records and tags remain unchanged, including equal
version numbers that came from different commits. They are not retroactively
declared equivalent. New shared versions advance beyond that historical module
history; old pending attempts remain recoverable through finalization.

### Shared Astro documentation websites

Astro + Starlight is the default for new documentation sites. The Rust CLI bundles the frontend and locked npm dependencies, prepares immutable Markdown/release/translation data, and invokes the shared frontend. Each consumer keeps its guides, configuration and optional presentation overrides. There is no separate npm package release to coordinate with the CLI.

In a repository with canonical guides under `docs/agents/`, a `docs/releases.md` runbook and installation guidance (`IMPORT.md`), install Node.js 24 with npm, initialize the documentation config, and build:

```sh
android-repo-docs init --repository owner/library --tooling-ref FULL_COMMIT_SHA
android-repo-docs bootstrap generate
sh ./docs.sh build --check
```

Use a published tag or immutable commit containing this backend as the tooling pin. Explicit `site.backend: astro` switches an existing built-in site; external command arrays must be removed intentionally when adopting the shared renderer. Existing Zola detection and external arrays retain their current backend until migration. See [configuration and compatibility](docs/standalone-docs.md#shared-astro-default-and-compatibility) and [the library migration audit](docs/astro-migration-audit.md).

### Zola documentation compatibility

Repositories opt in to the shared Zola backend by checking in `sites/zola.toml`. `android-repo-setup build_docs --check` then prepares immutable release/development content and runs Zola 0.23.6; new sites without that file use shared Astro. An explicit `site.backend` takes precedence over detection, and existing external documentation commands remain supported.

Install Zola 0.23.6 on PATH for local builds. Generated Documentation and release-validation workflows install its verified binary for opted-in sites and omit Node setup. Native CLI integration tests use the real Zola binary; CI installs it on both Linux and Windows. Neither Python nor Node.js is needed by the Zola site build.

Canonical English guides live recursively in `docs/agents/`. Reviewed Spanish counterparts live in `docs/es/`; `docs/es/translations.json` maps relative guide names to the SHA-256 of exact English source. Missing or stale translations visibly fall back to English at the selected documentation revision. Release archives and optional translation trees are read as data; historical scripts are never run.

The bundled theme in `cli/docs/site-theme/` provides light/dark modes, scoped search, responsive sidebar and on-page contents, version/language selection, source/documentation provenance, raw Markdown and screenshot galleries. Override templates in `sites/templates/` and assets in `sites/static/`. `versions.json` describes the generated scopes; old renderer links and root integration Markdown remain available. Demo Common is the reference integration.


#### Start a Zola site

Zola support is included in CLI v0.1.7. Pin the release in `.project/setup/config.yaml` and retain the rest of your project configuration:

```yaml
tooling_ref: 'v0.1.7'
workflows:
  # Keep your module, app_module and title settings here.
  documentation_command: android-repo-setup build_docs --check
```

Add `sites/zola.toml` (replace the repository URL and title):

```toml
base_url = "https://lambdawalker.github.io/android.apexfission.demo-common"
title = "Demo Common"
description = "Material 3 Compose components for Android demo catalogs."
default_language = "en"
build_search_index = true

[languages.es]
build_search_index = true

[search]
index_format = "fuse_json"

[markdown.highlighting]
light_theme = "github-light"
dark_theme = "github-dark"
```

With CLI v0.1.7 and Zola 0.23.6 on `PATH`, run from the Android repository root:

```sh
android-repo-setup workflows --overwrite --set-tooling-ref v0.1.7
android-repo-setup workflows --check
android-repo-setup build_docs --check
```

Review and commit generated workflow changes. Output is `sites/dist/`, ready for the generated GitHub Pages workflow. Canonical pages use `/<language>/<module>/<version>/` and matching `/raw/*.md` endpoints; `development` is visibly unreleased. New shared identities combine confirmed destinations under one version while preserving their installation coordinates. Historical provider-specific routes retain their original source commits. Version/language changes preserve the guide when available, and explain an overview fallback otherwise.

Use `--base <absolute-preview-url>` when previewing on another host, or `--base /path` to change the configured host's path prefix. Historical Git objects and optional translation trees must be available; fetch full history. `--check` validates generated local links, anchors and images. It does not check external website availability or render Android UI. See [Demo Common's website guide](https://github.com/lambdawalker/android.apexfission.demo-common/blob/main/sites/README.md) for the complete source/translation ownership rules and preview.


### Windows IDE terminal mouse input

On Windows, terminals identified by `TERMINAL_EMULATOR=JetBrains-JediTerm`
(or another `JetBrains` prefix) use keyboard navigation. Some IDE terminal / Windows
console combinations lose the Escape prefix of mouse reports before the CLI can
decode them, causing text such as `[MC…` to appear in fields. Startup disables both
native console and ANSI mouse tracking, including tracking left by an interrupted
run. Use Tab / Shift+Tab to change focus and Enter to activate a control.

For a terminal that is not automatically identified, opt into keyboard mode before
launching the wizard from PowerShell:

```powershell
$env:ANDROID_REPO_SETUP_NO_MOUSE = '1'
.\setup.bat
```

This setting requires a CLI build containing this fix; it has no effect in v0.1.21.
Until an updated build is installed, run setup in a separate PowerShell window.
Mouse support remains enabled in other Windows terminals. The decoder below is
still used there; keyboard mode avoids interpreting ambiguous report fragments as
text without filtering legitimate characters from passwords or paths.

The wizard normalizes legacy X10 and SGR mouse reports that some Windows IDE terminals, including IntelliJ IDEA, deliver as character events. Pointer movement does not alter text or password fields; clicks, dragging and vertical scrolling remain mouse actions. Native console events pass through unchanged. The compatibility layer applies on Windows only, before input reaches the views, and preserves ordinary text and keyboard shortcuts.

The input compatibility fix is included in CLI v0.1.8. The iocraft interface described above requires a release containing this migration, or a build from this branch.

## Android translation validation

Run `android-repo-setup --project /path/to/repository check_localization` (alias
`check-localization`) to validate named Android `<string>` resources without
Gradle, an Android SDK, or Python. The generated `verify-localization.yml` runs
the same check on pull requests and main. Regenerate workflows using a CLI
release containing this command; old pinned binaries do not support it.

The command discovers `src/<source-set>/res` directories throughout a repository,
including nested app/library modules, and reads all XML files under `values`
and locale-only directories such as `values-es`, `values-pt-rBR`, and
`values-b+es+419`. It skips symlinks, hidden, build, target, bin and node_modules
directories. Each source set is checked independently; resource roots outside
the repository or custom source directories are not discovered.

Checks reject malformed XML, duplicate names across files, extra keys, empty
translations, translations of `translatable="false"` strings, and incompatible
Java Formatter argument indices/types (including implicit positions, repeated
arguments, date/time conversions and escaped percent signs). Styled text and
XML entities are read as text. `formatted="false"` default strings skip format
comparison. Missing keys fail by default; pass `--allow-missing` to allow
Android's default-language fallback while retaining the other checks. No fixed
list of languages is imposed, and the check does not require adding locales.

This first version checks named strings, not plurals, string arrays, translation
quality, Android escape semantics, or resource-merging behavior. Device/API
qualifier overlays (for example `values-es-night`) are excluded rather than
treated as incomplete translations. It prints resource/locale counts, including
zero when no resources are found.

## Shared bootstrap and project checks

Generate maintained cross-platform launchers with `android-repo-setup bootstrap generate`.
Use `bootstrap check` for drift, and `setup check` to include publishing/workflow
configuration and evaluated Gradle inventory. See [bootstrap](docs/bootstrap.md).

Generic screenshot inventories, required JUnit cases, and isolated release-source
validation are configured in `.project/verification/checks.json`. See
[project checks](docs/shared-project-checks.md). Consumers retain only their
domain-specific documentation/media engines; shared helper tests live here.
