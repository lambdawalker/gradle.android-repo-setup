# Consumer infrastructure audit — 2026-11-13

Scope: Permissions #44, Demo Common #38, Auth #63, YOLO #8 and Coordinates #7. All five PRs are merged. The last three upgraded the documentation executable only; they did not migrate release infrastructure. This audit compares their main branches with shared tooling v0.1.20.

| Consumer | Existing integration | Migration result / remaining work |
|---|---|---|
| Permissions | Setup/docs v0.1.20; root/library plugin v0.1.5; no Python files | Upgrade plugin to v0.1.20; shared generated update action still embeds Python |
| Demo Common | Setup/docs v0.1.20; root/library plugin v0.1.9; no Python files | Upgrade plugin to v0.1.20; shared generated update action still embeds Python |
| YOLO | Docs v0.1.20; local Python release engines; handwritten Gradle publishing | Migrate setup, release workflows, JitPack and root/library plugin to v0.1.20; remove Python engine/tests; preserve Astro renderer and project Android checks |
| Auth | Docs v0.1.20 delegates site build to Python; local release/signing tooling | Release-verifier parity gaps below prevent safely removing the local engine; screenshot/geography/site tooling also remains |
| Coordinates | Docs v0.1.20; root Kotlin/JVM library; local Python release engine | Requires JVM discovery, plugin and JAR verification support before migration |

## Required shared capabilities

### JVM libraries and JAR publications (Coordinates)

- `src/main/kotlin/com/apexfission/release/AndroidRepoSetupPlugin.kt` requires `com.android.library` and always configures `AndroidSingleVariantLibrary`. Add a JVM-capable plugin or explicitly supported JVM mode using `KotlinJvm`/Java library publication.
- `cli/gradle/discover.gradle` discovers Android plugins only. Preserve the Coordinates release ID while mapping it to Gradle root `:` and directory `.`; do not change it to `root` or fabricate an Android module.
- `cli/src/release.rs` hardcodes `.aar` in `SUFFIXES`, requires POM packaging `aar`, and checks `AndroidManifest.xml`/`classes.jar`. Parameterize packaging throughout local/public verification, hashing, confirmation and journal contracts. Coordinates requires direct JAR class scanning and Java 11 bytecode (55).
- Generate JVM tasks (`test`, `runExamples`, JAR publication), without Android lint/SDK/APK/screenshot assumptions. Preserve existing guards, root-level task paths, coordinates and confirmed history.
- Prove with credential-free root-JVM integration tests for both Central and scoped JitPack coordinates, including negative publication selection and malformed JAR checks.

### Auth publication verification parity

Auth's `scripts/release.py` and `scripts/module_release.py` enforce requirements not fully expressible in v0.1.20:

- Exact pinned dependencies read from the version catalog, including implementation/runtime dependencies. Shared `required_api_dependencies` requires compile/API export; forcing Auth dependencies to `api` would change its contract. Add scope-aware dependency requirements and catalog selectors, or an equally strict declarative input mechanism.
- Models are extracted from a pinned published model AAR through Gradle, not committed Git assets. The shared asset contract currently supports `source: git` only. Add immutable Maven artifact provenance and selected-entry SHA-256/size/format verification, including JitPack repositories, before replacing Auth's reserved model hashes.
- Auth rejects `com/apexfission/android/identitymock/` classes and internal/unresolved dependencies. Add configurable forbidden class prefixes and forbidden dependency coordinates; retain Java 17 and required package checks.
- Keep Auth's explicit owner-selected license guard. Its unsigned CI uses a clearly identified validation-only license fixture; generated verification needs a way to pass those local Gradle properties without making them publication defaults.

Do not delete Auth's verifier or weaken these checks merely to complete migration. Its current build also contains old `android.attestra.auth` publication identity strings: reconcile repository naming explicitly before generating new coordinates; no confirmed Auth artifact exists.

### Remove inline Python from shared workflow updates (all migrated consumers)

`cli/workflow-steps/update-docs-pin.yml` and `refresh-docs-bootstrap.yml` invoke Python to edit/read the docs tooling pin. Replace with native CLI pin read/update/bootstrap operations with stable-tag/full-SHA validation, atomic writes, and preservation of unrelated configuration. Regenerate consumer workflows from the new release; do not patch each generated action independently.

### Remaining Auth development tools

- Screenshot suite separates coverage, candidates, reviewer acceptance, baseline restoration and documentation freshness. Shared manifest checks do not replace that full lifecycle. Add equivalent reviewed-media operations or keep this project-specific engine until a separately tested replacement exists. Never auto-accept stale captures.
- `sites/build.py`/`test_build.py` remain the actual site renderer behind `android-repo-docs`; migrate to the shared Astro backend with route/history/language/screenshot parity before deleting them. The renderer decision and per-library requirements are recorded in [the Astro migration audit](astro-migration-audit.md).
- Geography catalogue generation/tests are domain tooling, not publishing infrastructure. Port separately if zero Python in the entire repository is required.
- Signing/environment scripts and their tests still need migration to the shared wizard and generated signing workflow, preserving app signing verification and existing environment semantics.

## Validation and migration constraints

Permissions and Demo Common passed shared bootstrap, workflow, destination configuration and confirmed metadata checks. YOLO passed those checks after migration, plus its Astro 90-page build/link validation. The v0.1.20 Gradle plugin POM was available from JitPack (HTTP 200). Local Gradle execution was blocked downloading Gradle 9.6.0 with `Network is unreachable`; Android build and evaluated module-inventory verification must therefore run in CI. These results are not a claim of successful publication or device testing.

Keep pre-migration release source commits and journals intact. The shared verifier intentionally requires the contract from the reserved source; older pending attempts require their original verifier and a verified confirmation record, not current configuration substituted for immutable expectations.
