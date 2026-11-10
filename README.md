# Android repository setup Gradle plugins

These convention plugins complement [android-repo-setup](https://github.com/lambdawalker/python.android-repo-setup). Gradle builds and publishes the Android artifacts. The Python CLI continues to manage release reservations, verification against remote repositories, and documentation generation.

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

The library plugin configures the release AAR, sources and documentation jars, reproducible jar settings, and the local `verification` Maven repository. Existing workflows can continue to use `-PreleaseModule`, `-PreleaseVersion`, `-PjitpackBuild=true`, and `-PjitpackScopedTag=true`. Maven Central uploads require the configured credentials and a valid Python release reservation.

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

After merging the JitPack build configuration, create a new `vX.Y.Z` release tag on `main`. Run **Publish Gradle plugins to JitPack** with that tag. The workflow triggers JitPack through a Maven artifact request and verifies the build source and both plugin descriptors. The original `v0.1.0` tag predates the single-artifact JitPack configuration.

Consumers using JitPack can add `https://jitpack.io` to `pluginManagement.repositories` and map both plugin IDs to `com.github.lambdawalker:gradle.android-repo-setup:vX.Y.Z` with `resolutionStrategy.eachPlugin` and `useModule`.

## Standalone setup CLI

The Rust CLI owns workflow generation. Build it from the repository root:

```sh
cargo build --locked --release --manifest-path cli/Cargo.toml
```

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

The workflow command preserves the output of the Python renderer at
[`b6351aa`](https://github.com/lambdawalker/python.android-repo-setup/commit/b6351aa717d7ffcbc4ecf1dac9179669e072c361).
The generated workflows and the existing Gradle release guards still call
Python. Release coordination, GitHub environment setup, and the wizard and
vault are pending migration.


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
