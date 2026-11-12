# Android Association Export Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan.

**Goal:** Generate an Attestra-compatible public signing association file beside the local signing vault.

**Architecture:** Resolve explicit application IDs from config or remembered wizard answers, extract the selected certificate with keytool, and atomically export its SHA-256 identity after key creation/verification. Keep all backend approval in Attestra.

**Tech Stack:** Rust, existing serde/sha2/tempfile dependencies, JDK keytool.

**Spec:** `docs/superpowers/specs/2026-11-11-android-associations-design.md`

## Global Constraints

- Use the exact existing `packageName` / `sha256CertFingerprints` import schema.
- Store `setup-<id>.vault.associations.json` beside the vault, including path overrides.
- Preserve old vault compatibility and never export private signing material.
- Do not modify the backend, publish packages, or rotate existing keys.

## Review Focus

- Missing or incorrect application IDs must not silently derive from repository names.
- A failed certificate export must preserve the prior public file.
- Different signing environments must not overwrite each other's exports.
- Existing vaults and reused keys must regenerate the public file without key changes.
- Symlink/nonregular destinations must not allow overwriting unrelated files.

## Task 1: Export and integrate public signing identities

**Files:** Create `cli/src/associations.rs`; modify `cli/src/main.rs`, `cli/src/signing.rs`, `cli/src/vault.rs`, `cli/src/docs.rs`, `cli/src/workflows.rs`, `cli/tests/fixtures/config.yaml`, `.github/workflows/check.yml`, and `README.md`.

**Interfaces:** `associations::application_ids(root, saved)` resolves configured/default IDs;
`associations::write(path, ids, certificate)` validates IDs and writes the import array;
`signing::prepare_signing_files` creates/verifies the key and exports its association file.

- [ ] Add failing tests for the exact public schema/digest, invalid IDs, atomic replacement, path isolation and nonregular destinations; extend real-keytool coverage to creation and reuse/export failures.
- [ ] Run focused tests and confirm failures arise from missing export behavior.
- [ ] Implement certificate export, atomic public JSON writing, wizard defaults/persistence, summary and no-op guidance.
- [ ] Run focused tests, then the full locked CLI suite, formatting and diff checks. Document generation, re-export, output path and manual Attestra import.
- [ ] Review the complete diff, commit and open a pull request.
