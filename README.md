# Android repository setup Gradle plugins

These convention plugins complement [android-repo-setup](https://github.com/lambdawalker/python.android-repo-setup). Gradle builds and publishes the Android artifacts. The Python CLI continues to manage release reservations, verification against remote repositories, and documentation generation.

## Try it in an Android library

Check out this repository alongside the Android project and add the included build to the project's `settings.gradle.kts`:

```kotlin
pluginManagement {
    includeBuild("../gradle.python.android-repo-setup")
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

Make the plugin checkout available to the tagged build, for example as a pinned Git submodule referenced by `includeBuild`. Configure the JitPack install command for the selected module, for example:

```yaml
install:
  - ./gradlew --no-daemon :demo-common:publishJitPack
```

When JitPack sets `JITPACK=true`, the plugin reads `VERSION` and validates the canonical `module/vX.Y.Z-jitpack` tag against the checked-out commit. It also compares `GIT_COMMIT` when provided. The selected module publishes to Maven Local using the scoped JitPack coordinates. This entry point requires Gradle and Git, plus the project's Android build toolchain.

## Development

Use JDK 21 and run `./gradlew check` (`gradlew.bat check` on Windows). The wrapper uses Gradle 9.6.0. Demo Common is the reference for publication behavior. Validate an Android project's release tasks in that project after applying the plugins.
