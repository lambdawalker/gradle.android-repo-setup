# Release attempt reconciliation

Publication has two independent outcomes: the registry may accept an artifact even
when its Actions job fails. `module_release prepare` now reconciles all attempts for
the selected module and destination before choosing a version. Generated publishing
workflows also reconcile a reserved attempt after a failure. Startup reconciliation
covers cancellations and runner loss where a cleanup step could not execute.

| Evidence | Result |
| --- | --- |
| New Central reservation never passed the atomic upload guard, no artifacts | Archive the abandoned reservation; fence any late guard |
| Central `FAILED`, matching deployment ID/name, no artifacts, status checked again | Archive the exact pending/uploading objects |
| JitPack terminal failure for the reserved commit, no artifacts, status checked again | Archive the exact pending object |
| All public artifacts pass source/coordinate/content verification | Finalize metadata and history in an isolated checkout, without an upload |
| Central `PENDING`, `VALIDATING`, `VALIDATED`, or `PUBLISHING`; known active JitPack build | Keep pending and report `active` |
| Missing receipt, unknown status, HTTP/authentication error, partial artifacts, changed hashes or provenance | Keep pending and report `needs-review` |

An Actions failure, age, `tagNotFound`, missing deployment, or absence from Central's
`maven-metadata.xml` is never proof of provider failure. Legacy Central reservations
without the upload protocol marker cannot be automatically classified as pre-upload.
`abort-unconfirmed` uses the same reconciliation rules; it no longer clears an
upload-started attempt merely because its version is absent from Maven metadata.

## Commands

Run under the repository release concurrency lock. Local operators must ensure no
other release operation is running and have permission to push journal refs and
verified release metadata to `main`.

```sh
# Default destination is Maven Central. Preview makes no remote changes.
android-repo-setup module_release reconcile --module carddetector
android-repo-setup module_release reconcile --module carddetector --apply

# Restrict recovery to one or more exact versions.
RELEASE_REPOSITORY=jitpack android-repo-setup module_release reconcile \
  --module carddetector --version 0.1.9 --version 0.1.10 --apply
```

The JSON/Actions summary lists `eligible`, `published`, `archived`, `finalized`,
`active`, or `needs-review` for each attempt. Apply processes independent eligible
attempts but exits nonzero if any reviewed attempt remains active or uncertain.
Already-resolved attempts are a no-op. Only the selected module/destination is
changed. The explicit newer-version JitPack override still preserves unresolved
attempts when intentionally publishing separately.

Failed versions stay reserved at that destination, even when source inputs are
unchanged. Source and shared identity tags remain intact. Atomic pushes compare the
exact inspected journal objects; a competing upload guard or finalizer makes a stale
cleanup fail. Reconciliation never drops Central deployments or deletes source tags.
An older successful publication is added to history without replacing newer latest
metadata. Documentation deployment stays independently retryable and never uploads
Maven artifacts. The normal Central 15-minute environment delay is unchanged.

## Central receipts and verification

The pinned Vanniktech 0.37.0 plugin reports an upload receipt immediately after the
Portal accepts its bundle. `track-central-upload` streams the plain Gradle output
and records that deployment ID in the reserved Git journal using an expected-object
lease. The original source, coordinates, hashes, workflow run and workflow attempt
remain attached to the receipt. Generated pipelines preserve `pipefail` and retain
the full deployment log even when publication or receipt persistence fails.

Read-only Portal status calls use `MAVEN_CENTRAL_USERNAME` and
`MAVEN_CENTRAL_PASSWORD` (or the corresponding `ORG_GRADLE_PROJECT_mavenCentral*`
variables already used during upload). Credentials and response bodies are never
written to journals. Redirects are disabled. A lost receipt leaves an unknown
upload, which requires investigation; the CLI does not guess deployment identity
from a name or a failed workflow run. Verify the retained log and Portal before
recovering such a legacy attempt.

New reservations store verification schema `1` and the fully resolved verification
contract, including exact dependency pins, asset hashes, sizes, and validation
rules. Confirmation uses this snapshot; unsupported or incomplete schemas fail
closed. Legacy reservations continue to require the contract at their immutable
source revision. Existing artifact verification (including Central hash comparison
and signature presence) is preserved.

The status integration follows the [Sonatype Publisher API](https://central.sonatype.org/publish/publish-portal-api/).
The receipt format is pinned to the [Vanniktech 0.37.0 upload implementation](https://github.com/vanniktech/gradle-maven-publish-plugin/blob/0.37.0/plugin/src/main/kotlin/com/vanniktech/maven/publish/central/MavenCentralBuildService.kt).
