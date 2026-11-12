# Discover and configure Android modules

Available in the 0.1.11 source release. Publish/install the matching CLI and Gradle plugin before adopting the generated workflows. Existing `workflows.module` / `workflows.app_module` configurations still render their original workflow names until discovery is saved.

```sh
android-repo-setup                      # discover, select modules, then select setup operations
android-repo-setup discover             # JSON inventory; does not save setup files
android-repo-setup discover --save      # save discovered modules, preserve existing exclusions
android-repo-setup select-modules       # reopen the module checkbox menu
android-repo-setup workflows --check    # preview drift without writing
android-repo-setup workflows --overwrite
android-repo-setup publishing_config generate
```

Discovery runs the project's Gradle wrapper with a bundled init script and configuration cache disabled. It evaluates the project's normal build configuration but runs no build or publication task. Run only in a project you trust. The project must configure successfully with its normal JDK/Android SDK/dependencies. Discovery errors are reported and leave selections unchanged; the CLI does not guess module types from folder names or build-file text.

Applied `com.android.library` and `com.android.application` plugins determine type. Kotlin/Groovy build scripts, version-catalog aliases, convention plugins, nested projects, root Android projects, and remapped project directories are resolved by Gradle itself. Non-Android projects are omitted. Included/composite builds are separate build roots: run `--project` against each root. Project directories outside the selected root are rejected.

The first screen selects all detected Android modules. Later runs restore selections by Gradle path and select newly discovered modules. Cancel leaves configuration unchanged. Unchecking a module excludes its setup and generated workflows, not its Gradle project or dependency relationships. Library-only selections omit signing operations; app-only selections omit library publishing operations.

For a new checkout, saving can bootstrap `.project/setup/config.yaml` from a GitHub `origin` remote. The CLI does not rewrite Gradle build scripts: apply the publishing plugin to each selected library and the root plugin as described in the main README, retaining POM metadata and any project-specific pinned dependency logic. Configure `GROUP`, `JITPACK_GROUP`, and per-module artifact IDs before releasing.

## Inventory and paths

```yaml
repository: example/android-tools
title: Android Tools
tooling_ref: v0.1.11
modules:
  libraries--vision: Vision
  model: Bundled Model
android_modules:
  libraries--vision:
    path: ':libraries:vision'
    directory: components/vision
    kind: library
    selected: true
    namespace: com.example.vision
    screenshots: false
  model:
    path: ':model'
    directory: models/bundled
    kind: library
    selected: true
    namespace: com.example.model
    screenshots: false
  phone:
    path: ':apps:phone'
    directory: mobile/phone
    kind: application
    selected: true
    application_ids: [com.example.phone, com.example.phone.debug]
    screenshots: false
workflows:
  title: Android Tools
  java_version: 21
```

Release IDs are distinct from task paths and directories. Defaults turn `:libraries:vision` into `libraries--vision`; the root project uses `root`. Existing IDs are retained when rediscovering the same path. IDs must remain stable once released because tags/history use them. You can set distinct IDs in `android_modules` before first publication if an ID or generated filename collides. Paths and IDs use letters, digits, dots, underscores and hyphens, with colons separating Gradle path segments. Directory paths may contain spaces.

`modules` retains library documentation labels, including deselected historical libraries. `android_modules` controls current setup selection. Saving selections and workflow generation also write `.project/setup/modules.properties` for the Gradle plugin; commit it with the YAML and workflows. `workflows --check` detects a stale inventory. Do not edit the generated properties manually.

Each selected library gets `publish-<id>.yml`, `verify-release-tooling-<id>.yml`, and `reconcile-failed-jitpack-<id>.yml`. Finalization offers all selected libraries. Shared publishing/finalization jobs serialize repository writes. Each app gets `build-signed-<id>-apk.yml`; apps currently share the existing `demo-apk-release` signing environment/key. Discovered variant application IDs provide signing association defaults, unless `signing.application_ids` is explicitly configured. Review package IDs when exporting associations.

Screenshot workflows are generated only for modules with the screenshot plugin. Unlike legacy single-module configuration, discovery does not assume five reference images. Module-specific screenshot policy can be added to the generated workflow, with local edits protected during updates. APK workflows retain the existing expectation of one unsigned, unflavored release APK; flavored/split APK output requires a project-specific workflow.

Overwriting workflows removes only obsolete files tracked in the generation manifest whose content still matches its stored hash. Locally changed obsolete workflows require explicit manual archival/removal. Unmanaged legacy workflows are never deleted automatically: remove superseded handwritten publishing entry points during migration to avoid duplicate actions.

## Independent library contracts

```properties
GROUP=com.example.android
JITPACK_GROUP=com.github.example
POM_ARTIFACT_ID.libraries--vision=vision
POM_ARTIFACT_ID.model=bundled-model
modelCoreArtifact=vision
modelCoreVersion=1.2.3
```

```yaml
module_verification:
  libraries--vision:
    class_prefix: com/example/vision/
    required_api_artifacts: []
    forbidden_asset_suffixes: [.tflite]
  model:
    class_prefix: com/example/model/
    required_api_artifacts: []
    required_api_dependencies:
      - group_property: GROUP
        artifact_property: modelCoreArtifact
        version_property: modelCoreVersion
    required_assets:
      directory: models/bundled/src/main/assets
      suffix: .tflite
fingerprint_excluded_properties:
  libraries--vision: [modelCoreVersion, modelCoreArtifact, modelVersion]
```

A module contract replaces the global `verification` fallback. Discovery initializes namespace-based contracts with an empty required API list when no contract exists; review these before release. `bytecode_major` defaults to 61 (Java 17). Exact dependencies also accept literal `group`, `artifact`, and `version` instead of property selectors. They must be available as a POM and its declared JAR/AAR on Maven Central before local release verification proceeds. The model's POM must export the exact dependency at compile scope; Gradle API variants must carry that same pin. This does not rewrite the dependency declaration: the model build must compile against its stable published dependency during release.

Asset paths and SHA-256 hashes come from the immutable source commit. The verifier rejects missing, changed, empty, duplicate, or extra assets with the required suffix; a core contract can forbid model assets. Resolved verification facts are recorded alongside artifact hashes. Local/public verification reads the source revision's contract, preserving reproducibility.

Fingerprint exclusions are exact Gradle property names. Exclude only properties that cannot affect that library's artifact. Shared build/tooling changes still conservatively change release fingerprints; model-only sources and explicitly excluded pins do not change the core fingerprint. Existing fingerprints retain their legacy algorithm when no new options are present.

## Card detector migration

Use the detected `carddetector`, `tfmodel`, and `app` entries, and map the two artifact IDs using `POM_ARTIFACT_ID.carddetector=card-detector` and `POM_ARTIFACT_ID.tfmodel=card-detector-model`. Give `tfmodel` its own dependency/asset contract as above, with `tfmodel/src/main/assets` and the existing `modelCoreVersion` / `modelCoreArtifact` properties. Preserve its conditional published-core dependency declaration.

Do not rename confirmed release records or tags. The historical `core` and `sentinel-card-model` coordinates remain facts about old releases. Keep the existing `core:0.1.0` model pin until an intended new core publication is confirmed. Migration of an in-flight Python release should finish using its original tooling; adopt the new workflow for the next release source. New tooling is not retroactively present at old immutable source commits.
