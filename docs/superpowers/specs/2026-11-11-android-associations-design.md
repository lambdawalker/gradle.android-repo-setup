# Android signing association export

The signing CLI must supply the public handoff consumed by Attestra's existing
Android association importer. The user requested creation alongside the signing
files under `~/.projects/<repo-name>/secrets/`.

After creating or verifying a local signing keystore, export the selected alias's
DER certificate with keytool and hash its bytes using SHA-256. Write an array of
`packageName` / `sha256CertFingerprints` objects, using uppercase colon-separated
fingerprints. Include no passwords, keys, repository metadata, or deployment
approvals. The Go renderer continues to own the hosted assetlinks document.

Use `setup-<id>.vault.associations.json` beside `setup-<id>.vault`, honoring existing
state directory and explicit vault path overrides. Replace this generated snapshot
atomically after successful certificate extraction; never accumulate retired keys.
The destination importer remains responsible for approval and merging.
Append the suffix to the complete vault filename so custom vault extensions remain
distinct. Reject an export destination equivalent to the selected keystore before
creating or modifying any signing files.

Application IDs come from optional `signing.application_ids` in project config or
the wizard's remembered answers. Prompt when absent; show the IDs and output path
before applying. Never infer an application ID from the repository or namespace.
Existing vaults remain readable. A GitHub-only no-op cannot inspect existing
remote secrets; direct users to verify their local key to generate the export.

Approach: export a public JSON file in the CLI. Reading private vaults from Go
would couple the backend to signing credentials; parsing human-readable keytool
fingerprints would depend on locale. DER hashing avoids both problems.

Scope: Rust CLI, tests and usage documentation. No backend edits, live deployment,
key rotation, package publication, or dependency additions.
