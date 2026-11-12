# Shared Release Identity Implementation Plan

> Agentic workers: execute the approved design task by task, with test-first implementation and whole-branch review.

**Goal:** Enforce one source/build identity per module/version across destinations.

**Architecture:** An immutable annotated identity tag joins atomic provider reservation; destination journals retain independent status and hashes. Canonical records carry `release_identity` metadata into merged documentation history; legacy records stay separate.

**Tech stack:** Existing Rust CLI, Git atomic pushes/create-only leases, JSON, Zola.

**Spec:** ../specs/2026-11-11-shared-release-identity.md

## Global constraints and review focus
- Preserve historical identities and provider routes; never infer equivalence from equal version strings.
- Check concurrent first reservations, changed inputs, source/config drift, uncertain uploads, and malformed/missing canonical tags.
- Keep destination-specific byte hashes and signing separate from canonical source identity.
- Use credential-free Git fixtures and fake registry responses; do not publish Android artifacts.

## Tasks
- [x] Core owner: add `release_identity.rs`; integrate `journal.rs`, `release.rs`, `confirmation.rs`, and module declaration in `main.rs`. Add failing Git-remote regressions for shared selection, atomic competing reservations, immutable canonical reuse, retries/override paths and legacy recovery; implement and run targeted tests.
- [x] Archive owner: update `docs.rs` and `docs/history.rs` to validate and merge canonical records into one version, preserving legacy routes. Add archive tests for both destination confirmations and conflicting identities.
- [x] Integration owner: inspect/regress workflow source/tooling ordering and safe retry behavior; update shared README. Prepare paired Demo Common integration with an exact tooling commit pin, regenerated workflows, bilingual owner guidance and unchanged historical release files.
- [x] Run locked CLI suite with Zola, formatting/diff checks, review changes, resolve findings and prepare tooling/consumer PRs with validation and release guidance.

Validation: all 95 locked CLI tests pass with Zola 0.23.6, along with formatting and diff checks. Independent review found a historical-coordinate compatibility regression; its test reproduced the failure before the fix and the complete suite passes afterward. Consumer pinning follows creation of the verified tooling commit.
