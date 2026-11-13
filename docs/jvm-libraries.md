# JVM library publishing

The CLI can discover and publish root or nested Kotlin/JVM and Java libraries.
This support is on the JVM feature branch; use its reviewed commit or a release
containing it. Updating the CLI does not update a project's Gradle plugins.

The release engine, destination registry, immutable source identities, annotated
journals, upload guard, reconciliation and finalizer are shared with Android.
JVM publications select JAR verification rather than introducing a second journal
format. Existing Android configurations continue to default to AAR verification.

## Project configuration

Discovery recognizes `org.jetbrains.kotlin.jvm` and `java-library` through the
evaluated Gradle model. Android application/library identities take precedence.
Core Gradle `application` projects are excluded from JVM library discovery.
Root projects use Gradle path `:` and directory `.`. The existing configuration
key `android_modules` is retained for compatibility; it can now contain
`kind: jvm-library` alongside Android modules. No Android SDK, namespace,
application ID, APK, Android signing certificate or screenshot configuration is needed
for a project containing only JVM libraries.

Example `.project/setup/config.yaml` for Coordinates:

```yaml
repository: lambdawalker/android.apexfission.math.coordinates
title: Coordinates
tooling_ref: <reviewed-CLI-commit-or-release>
modules:
  coordinates: Coordinates
android_modules:
  coordinates:
    path: ':'
    directory: '.'
    kind: jvm-library
    selected: true
workflows:
  module: coordinates
  title: Coordinates
  java_version: '17'
module_verification:
  coordinates:
    packaging: jar
    class_prefix: com/apexfission/android/math/
    bytecode_major: 55
    jvm_version: 11
    required_api_artifacts: []
    require_kotlin_stdlib: true
    forbidden_dependency_group_prefixes: [androidx.compose]
```

The verification contract must be explicit: discovery cannot determine the public
package prefix or intended bytecode target. `bytecode_major: 55` means Java 11
class files; `jvm_version: 11` checks Gradle library variant metadata. Both API and
runtime library variants must exist. Documentation variants are not executable
library variants. Kotlin profiles require the Kotlin `jvm` platform attribute;
plain Java profiles may omit it when `require_kotlin_stdlib` is false, but an
explicit incompatible platform is always rejected. Source JARs may contain
Kotlin or Java source. Configure class/package and dependency requirements for
the actual library; do not copy Coordinates' package into another project.

Keep the standard `GROUP` and `POM_ARTIFACT_ID` Gradle properties (or the existing
per-module artifact property) and configure Central/JitPack environments through
the existing setup repository registry. Run discovery and review its inventory,
then generate the existing workflow set:

```sh
android-repo-setup discover --save
android-repo-setup workflows --check
android-repo-setup workflows --overwrite
android-repo-setup publishing_config generate
```

The generated JVM publishing checks use scoped `test`, `build` and
`publishAllPublicationsToVerificationRepository` tasks. JitPack uses the selected
project's `publishToMavenLocal`. A root project resolves to `:test`, `:build` and
`:publishToMavenLocal`. Mixed Android/JVM builds dispatch validation by selected
module; Android modules retain their Android tasks. JVM-only builds omit APK,
screenshot and Android localization workflows.

## Gradle integration boundary

This change extends the CLI, not the Android Gradle publication plugin. Do not
apply the existing `com.apexfission.android-repo-setup` library plugin to a JVM
project: that plugin still requires `com.android.library`. Likewise, its Gradle
`ModuleInventory.selectedLibrary()` helper remains Android-only. A JVM project
retains its Java/Kotlin and Maven publishing configuration and POM metadata.

Before switching its workflows, provide the task/property contract already used
by the shared release engine:

- Accept `releaseVersion`, `releaseModule`, `jitpackBuild` and `jitpackScopedTag`.
- Publish the selected module's POM, primary JAR, sources JAR, documentation JAR
  and Gradle module metadata under `build/verification-repository/` at the build
  root. Preserve the JitPack repository-level artifact and consumer version rules.
- Keep `publishToMavenLocal` and `publishAndReleaseToMavenCentral` available on the
  selected project. The generated Central workflow invokes the latter task.
- Guard actual Central upload tasks with `android-repo-setup module_release guard`
  for the same selected module/version. Local verification must not invoke the
  public upload guard. Preserve signing and upload receipt capture.
- Remove old Python entry points only after equivalent local and recovery checks
  pass. The CLI does not rewrite the JVM build script during discovery.

Coordinates already implements these publication tasks and properties. Its next
migration replaces the remaining Python calls with these CLI commands; this CLI
branch does not modify Coordinates or publish a library.

## Verification and compatibility

New JAR reservations include `.pom`, `.jar`, `-sources.jar`, `-javadoc.jar` and
`.module` hashes, plus the immutable resolved verification snapshot. Source and
documentation JAR entry hashes are retained separately so an independently built
JitPack archive can be checked despite ZIP metadata differences. Duplicate archive
entries, corrupt content, wrong package/bytecode and incompatible dependencies
are rejected. JitPack may omit `.module`; Central requires all recorded artifacts
and signatures. Presence checks fence partial `.jar` or `.aar` publications.

Old AAR snapshots remain valid with their original defaults. Older JAR records
are recognized from their exclusive primary artifact hash without rewriting the
journal or inventing a source identity. Confirmation cannot change JAR into AAR,
or the reverse. Root JVM fingerprints exclude generated installation pointers,
release archives and translations; source changes still change the fingerprint.

A historical reservation without a verification snapshot still needs the contract
from its immutable source. If that source predates setup metadata, the CLI refuses
to substitute today's configuration. Verify using the original release tooling,
then pass the exact verified confirmation record to the current finalizer, or
finish that attempt before migrating. Preserve its pending/uploading tags and
artifact hashes. Original unscoped paired-release journals remain on their
original recovery workflow; this feature does not reinterpret them as new JVM
reservations. Published version/source tags and confirmed histories remain intact.
