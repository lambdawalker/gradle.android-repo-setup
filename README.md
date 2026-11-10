# Android repository setup Gradle plugins

Gradle builds and publishes Android artifacts. The companion Rust CLI configures the project, manages release reservations, verifies public artifacts, and generates documentation.

## Use a published release

After version 0.1.0 is available on the Gradle Plugin Portal, add the following
to the root `build.gradle.kts` plugins block:

```kotlin
plugins {
    id("com.apexfission.android-repo-setup.root") version "0.1.0"
    id("com.apexfission.android-repo-setup") version "0.1.0" apply false
}
```

Apply the library plugin in each publishable Android module as shown below.
Keep `gradlePluginPortal()`, `mavenCentral()`, and `google()` available in
the consumer's plugin repositories.

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

Use **Actions → Publish CLI and JitPack** to publish
both in parallel from one project tag. It uses the same tag selection rules.
The original `v0.1.0` tag predates the single-artifact JitPack configuration.

Consumers using JitPack can add `https://jitpack.io` to `pluginManagement.repositories` and map both plugin IDs to `com.github.lambdawalker:gradle.android-repo-setup:vX.Y.Z` with `resolutionStrategy.eachPlugin` and `useModule`.

## Standalone setup CLI

Build the Rust CLI from the repository root:

```sh
cargo build --locked --release --manifest-path cli/Cargo.toml
```

To download prebuilt executables, use the CLI releases on GitHub. Each release
includes a Windows x64 ZIP and Linux x64 and Apple Silicon macOS tarballs,
each with its own SHA-256 checksum. Extract the archive and put the executable
on `PATH`. The APK and Maven signing commands use `keytool` and GnuPG,
respectively.

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
upload artifacts. The current artifact contract follows Demo Common, including
its exported Compose dependencies and Java 17 bytecode.

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
