# Android module discovery and independent publishing

## Accepted design
Discover Android modules from evaluated Gradle plugins, show a first checkbox menu with all modules initially selected, and persist selections. Support arbitrary names, nested Gradle paths and remapped project directories. Keep existing single-module configuration compatible. Libraries publish separately; apps get separate APK workflows using the existing shared signing environment. Discovery does not publish anything or edit dependencies. Failed discovery must not silently produce authoritative results.

## Implementation plan
1. Add a Gradle init-script discovery provider and typed module inventory, stable release IDs, selection persistence, command-line inspection and selection, and regression tests for arbitrary/nested/remapped modules, exclusions, malformed reports and failed Gradle configuration.
2. Add an iocraft module checkbox screen before setup operations; filter operations by selected module kinds. Persist selections before credential setup; preserve cancelled selections.
3. Generate dedicated publish and APK workflows; preserve legacy templates; support module paths/directories independently, app-only/library-only repositories, disabled modules, and obsolete managed workflow removal with local-edit protection.
4. Add per-library verification contracts for exact dependency pins and model asset hashes; support module-specific property fingerprint exclusions and documentation packaging; retain separate release records and guards.
5. Document configuration and migration; run Rust tests/format/lint, Gradle checks and actual Gradle discovery fixtures; review and open a PR.

## Review focus
- Convention plugins and aliases require evaluated Gradle, not text matching.
- A disabled sibling must never be published by generated setup.
- File paths must not be confused with Gradle task paths or release IDs.
- Existing release metadata and immutable tags remain untouched.
- Discovery failure/cancellation must not overwrite saved selections.

## Completion evidence
- Implemented all five tasks; source versions prepared as 0.1.11 without publishing.
- Reviewed the full branch independently. Fixed stale selection authorization, Groovy/convention build fingerprint inputs, YAML-like module IDs, JAR dependency verification, and demo-only screenshot defaults.
- Module selection updates both YAML and the Gradle inventory immediately. Publication CLI additionally rejects stale inventories and deselected libraries.
- Independent release records/tags remain untouched; existing handwritten consumer workflows require migration as documented.
- Rust suite passed with the pinned Zola binary installed. Real Gradle 9.6 discovery fixture passed. Kotlin source/test compilation and JUnit passed using pinned dependencies; normal Gradle dependency resolution was unavailable in this environment.
- Lint passed with three pre-existing lint categories allowed (unnecessary_unwrap in Zola, Windows test dead_code, manual_range_contains in mouse decoding). No new lint suppression was added to the codebase.
