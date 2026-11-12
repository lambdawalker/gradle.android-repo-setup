# Release assets

The next CLI release adds reusable Git-backed asset contracts for models,
dictionaries, databases, and other files bundled in an Android AAR. The CLI
prepares and verifies files; the Android build remains responsible for packaging
those files at the configured archive paths.

Configure each selected library in `.project/setup/config.yaml`:

```yaml
module_verification:
  tfmodel:
    required_api_artifacts: []
    assets:
      - directory: tfmodel/src/main/assets
        include: ['**/*.tflite']
        source: git
        hydrate_lfs: true
        validator: tflite
      - directory: tfmodel/src/main/assets/dictionaries
        include: ['*.txt']
        archive_directory: assets/dictionaries
        required: false
```

Keep the module's existing `class_prefix`, `required_api_dependencies`, and other
verification fields. A module contract replaces the global fallback contract.
The example's two sets deliberately select different file types: mapping two
files to the same AAR path is rejected, even if they have identical bytes.

| Field | Default | Meaning |
|---|---|---|
| `directory` | Required | Repository-relative source directory |
| `include` | `['**/*']` | Patterns relative to that directory; `*`, `**`, and `?` supported |
| `source` | `git` | Expectations are derived from the committed Git objects |
| `hydrate_lfs` | `false` | Allow downloading missing/unresolved Git LFS assets |
| `required` | `true` | Require at least one matching committed file |
| `allow_empty_files` | `false` | Permit zero-byte assets (format validator still applies) |
| `validator` | `none` | `none` or `tflite` |
| `archive_directory` | `assets` | Destination prefix within the AAR's `assets/` directory |

`**/*.tflite` matches both root-level and nested files. Patterns are case-sensitive;
brace expansion, character classes, negation, and exclusion patterns are not
supported. Paths support ASCII letters, digits, spaces, `_`, `-`, and `.` within
normal path segments. Absolute paths, traversal, symlinks, and wildcard path
names are rejected. Use forward slashes on all platforms.

## Preparation and verification

```sh
android-repo-setup assets prepare --module tfmodel
# Automation may additionally assert the full checked-out commit SHA:
android-repo-setup assets prepare --module tfmodel --source "$(git rev-parse HEAD)"
```

The command reads the contract and source files from HEAD, and prints a JSON map
of AAR entry names to source paths, SHA-256 hashes, and sizes. Worktree edits to
the contract cannot weaken these expectations. Commit configuration changes
before running preparation. The selected module must be a library.

For ordinary Git blobs, hashes and sizes come from those blobs. For Git LFS,
they come from the committed pointer's object ID and size, never the pointer's
own hash. Only missing files or unchanged pointers may be hydrated; other local
modifications fail verification. Hydration requires `git-lfs` and access to the
repository's LFS remote, and fetches only the selected paths. Filter settings
are supplied to the Git subprocess without changing repository/global Git
configuration or hooks. Standard LFS v1 SHA-256 pointers are supported; extended
or malformed pointers are rejected.

Preparation rejects unexpected matching worktree files, missing required sets,
empty files (unless allowed), changed bytes, and unresolved LFS pointers. An
optional set may contain zero committed files; it does not permit arbitrary
untracked files. `tflite` checks the `TFL3` identifier in the header. It does not
execute the model or prove tensor compatibility, accuracy, or runtime behavior.
There are no external URL sources or user-supplied validation commands.

Local and public release verification share the resolved contract and validate
AAR entry presence, hashes, sizes, and optional format checks. Extra matching
entries and duplicate ZIP paths fail. Resolved facts are included with release
verification metadata. Existing forbidden suffix checks continue to apply.

## Workflows and JitPack

Regenerate workflows with a CLI version containing this feature:

```sh
android-repo-setup workflows --overwrite --set-tooling-ref <released-tag-or-commit>
```

Generated Central, JitPack, and release-tooling verification workflows prepare
the selected module before building. `android-repo-setup jitpack_build` also
prepares assets before calling Gradle. Install the pinned CLI in the JitPack
build environment before using that entry point.

A direct Gradle `publishJitPack` / `publishToMavenLocal` invocation does not call
this CLI preparation command. If retaining that entry point, run `assets prepare`
explicitly before Gradle, or use `android-repo-setup jitpack_build`. No Gradle
plugin change is required for the asset contract: it governs input preparation
and verification of the resulting AAR, while existing per-module publication
configuration remains responsible for packaging.

## Migration

Existing `required_assets: {directory: ..., suffix: .tflite}` remains accepted,
with the same required/nonempty behavior. It now resolves LFS expectations from
pointer metadata but does not automatically enable hydration. Replace it with
`assets` to opt into hydration or format validation. Using both forms in one
contract is rejected.

For carddetector, migrate `tfmodel` using its existing asset directory, keep the
exact published-core dependency contract, and enable `hydrate_lfs` and `tflite`.
Keep the consumer's Python helper until a released CLI containing this feature
is installed and its JitPack bootstrap invokes the new path. Existing immutable
release tags must retain their original build setup.
