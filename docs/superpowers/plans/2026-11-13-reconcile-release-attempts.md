# Release attempt reconciliation implementation plan

> **For agentic workers:** Use superpowers:executing-plans to implement these tasks with tests first.

**Goal:** Failed deployments no longer block newer releases when their failure can be proved; successful uploads can finish bookkeeping without re-uploading.

**Architecture:** Extend the durable Git release journal with Central's deployment ID and a versioned verification snapshot. A provider-aware reconciler checks public artifacts and provider state, then uses existing atomic ref leases and finalization. Generated workflows reconcile before version selection and after failure, and capture the Central ID from the pinned publishing plugin's upload output.

**Tech Stack:** Rust CLI, Git atomic pushes, Sonatype Publisher API, JitPack API, generated GitHub Actions YAML.

**Spec:** This document records the design approved in the conversation on 2026-11-13.

## Global constraints

- One module and destination per invocation; preserve sibling attempts, immutable source tags, release identities, and failed-version floors.
- Never infer provider failure from a failed Actions run, elapsed time, HTTP error, missing deployment, or missing Maven metadata.
- Archive only a fenced pre-upload reservation or an identity-matched terminal provider failure with no public artifacts. Unknown, partial, and active deployments stay pending.
- Published artifacts must pass the existing complete verification before finalization. Documentation deployment stays separately retryable; no upload during reconciliation.
- Retain release concurrency locks and Central's normal 15-minute environment delay.
- Prepare PRs; do not merge or republish released versions.

## Review focus

- Upload guard racing with pre-upload cleanup: exactly one transition wins.
- Provider status switches or returns mismatched identity: preserve the reservation.
- Lost capture, authentication errors, and incomplete public artifacts: report unknown, never archive.
- Multiple attempts and newer confirmed history: process exact versions without overwriting newer metadata.
- Historical verification input/schema changes: use immutable resolved snapshots for new reservations, source-based checks for legacy ones.

### Task 1: Durable evidence and verification snapshots

Files: `cli/src/central.rs`, `cli/src/journal/reconcile.rs`, `cli/src/release.rs`, `cli/src/confirmation.rs`, `cli/src/main.rs`.

- [x] Add failing tests for Central status identity/state, exact upload-line parsing, durable ID capture with leases, and resolved snapshot round trips.
- [x] Run focused tests and observe failure; implement `track-central-upload` and the authenticated read-only status client.
- [x] Add snapshot schema 1 and workflow attempt to new reservations. Confirm using snapshots; keep source-based legacy fallback and reject unknown schemas.
- [x] Run focused tests, commit.

### Task 2: Provider reconciliation

Files: `cli/src/journal/reconcile.rs`, `cli/src/journal.rs`, `cli/src/confirmation.rs`.

- [x] Add failing tests for no-upload archival, terminal Central failure, public/partial/active/unknown cases, multiple JitPack attempts, repeatability and races.
- [x] Implement `module_release reconcile --module ... [--version ...] [--apply]` for both providers, with a JSON report.
- [x] Reuse full verification and finalize published releases in an isolated worktree; retain newer confirmed metadata.
- [x] Route Central abort-unconfirmed through the same proof. Reconcile before version selection; preserve explicit JitPack override semantics.
- [x] Run focused tests, commit.

### Task 3: Workflows and adoption

Files: publish templates, CLI documentation, card detector setup pin and generated workflows.

- [ ] Add failing workflow contract tests; capture Central upload ID in the streaming pipeline; reconcile after unsuccessful uploads/builds, preserving failure exit status and logs.
- [ ] Generate card detector workflows with an immutable CLI commit pin, avoiding dependency on an unreleased version label.
- [ ] Run complete Rust suite, formatting and workflow generation checks. Review the branch independently and resolve material findings.
- [ ] Open shared CLI and card detector PRs with evidence and rollout notes.
