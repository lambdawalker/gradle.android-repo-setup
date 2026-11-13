# Documentation media

Standalone `android-repo-docs` never needs setup, signing secrets or an Android SDK for accepted-media checks. Aggregate `check` validates configured accepted media only. It never invokes a renderer or requires fresh candidates. Acceptance is always explicit; review candidate images before running it.

Configuration lives in `.project/docs/config.yaml`. Paths are repository relative; `.` is allowed as a base. Absolute paths, traversal and symlinks are rejected. Commands are literal argv arrays run at repository root, without shell interpolation. `./gradlew` becomes `gradlew.bat` on Windows.

```yaml
media:
  manifest: docs/screenshots/manifest.json
  base: docs/screenshots
  required_ids: [historical-example]
  source:
    path: docs/demo.mp4
    min_bytes: 1000000
  frames:
    - id: historical-example
      candidate: build/docs-media/example.png
      evidence: build/docs-media/example.json
      command: [ffmpeg, -hide_banner, -loglevel, error, -y, -ss, '12', -i, docs/demo.mp4, -frames:v, '1', -vf, 'scale=540:-1', -threads, '1', build/docs-media/example.png]
  manifests:
    - path: docs/screenshots/ui-manifest.json
      base: docs/screenshots
      collection: images
      file_key: file
      hash_key: sha256
      required_files: [example.png]
      require_source: true
      require_dimensions: true
screenshots:
  manifest: docs/screenshots/manifest.json
  base: docs/screenshots
  candidates: library/src/screenshotTestDebug/reference
  input_paths: [library/src/main, library/src/screenshotTest, library/build.gradle.kts, gradle, gradle.properties, build.gradle.kts, settings.gradle.kts]
  command: ['./gradlew', ':library:updateDebugScreenshotTest', --stacktrace]
  legacy_base: sites/static/screenshots
  provenance: 'Locally rendered candidates accepted after review.'
  evidence: build/documentation-screenshots/evidence.json
```

All fields shown under `media` except `manifests` may be omitted for manifests-only validation. Additional manifests have a JSON array at `collection`; `file_key` defaults to `file`, `hash_key` to `sha256`, a nonempty `required_files` is the exact expected filename set (missing and extra entries fail), while an omitted or empty list allows any unique filenames, and source/dimension requirements default to false. Every entry must have a matching SHA-256 and a decodable PNG. `require_source` requires a full lowercase 40-character SHA at top-level `source`. Dimensions compare decoded width/height.

The primary manifest uses a nonempty `scenarios` array with unique `file` and, when supplied, unique `id` and `preview`. Every scenario has `sha256`. Optional `fixture`, `guide`, and `demo` paths must exist. Recording provenance is `recording.sha256`; it must match configured source bytes. Historical recording metadata, unknown fields and authored wording remain intact on acceptance. If recording source bytes change, review and explicitly update the manifest recording hash and authored provenance before rendering; the engine never infers replacement historical capture metadata.

`media verify` checks source and accepted manifests. `media render` runs configured frame commands and writes candidate evidence. `media compare` renders then compares exact accepted bytes. `media accept` prevalidates all candidates and evidence before copying any accepted asset and updating hashes. Before rendering, all candidate/evidence paths are checked against source inputs, accepted images, manifests, configuration, and one another. Old candidate/evidence files are then removed, and the command must produce a fresh decodable PNG. Evidence binds source bytes, output bytes and the complete media configuration. Accepted render metadata records the literal command and executable. Frame metadata (timestamp/filter) remains authored in the existing manifest; keep it consistent with command configuration.

`screenshots manifest` checks accepted images and optional legacy byte copies. Each screenshot scenario additionally requires nonempty `alt`, `preview`, and `capture_kind`. `screenshots render` first rejects overlap in either direction with accepted and legacy bases, actual accepted files, source inputs, manifests, or configuration; it then clears the configured candidate directory, runs the command, verifies inputs stayed constant, and writes `capture.json`. `screenshots check [directory]` compares decoded RGBA pixels. `screenshots update [directory]` prevalidates every candidate, then replaces accepted/legacy bytes and updates scenario capture metadata. No check/render command accepts images. Optional `screenshots.evidence` writes comparison JSON for check/update with current `source_commit`, `input_sha256`, `mode`, `compared` filenames and `differences`. Check validates all candidates first, collects every pixel mismatch, and writes the report before failing. Evidence paths must not overlap accepted files, configuration, source inputs, manifests or candidate outputs.

Preview method names match `[A-Za-z][A-Za-z0-9]*`. Candidate PNG paths must match a method at the start or after a slash/dot, followed by underscore, dot, space, slash, or end. Each method resolves exactly once; multiple scenarios cannot share a capture. Extra PNGs are allowed because renderers can emit additional unrelated previews.

Screenshot input hashes preserve the legacy algorithm: Git cached and untracked nonignored paths selected by `input_paths`, deduplicated and sorted, then SHA-256 of each UTF-8 filename, NUL, and raw file bytes; tracked deletions contribute `<deleted>`. Fresh capture provenance requires full `source_commit`, matching `input_sha256`, and `config_sha256` (SHA-256 of compact, sorted-key JSON configuration). Configuration digest is intentionally additional to the legacy input hash. Old capture files must be rerendered before check/update; accepted historical manifests need no migration. Render captures also record dirty state, OS, architecture, and command. Explicit update preserves unknown top-level/scenario metadata.

`screenshots inventory --module library` uses the existing `.project/verification/checks.json` `screenshots.library` contract (`directory`, `scenarios`, optional `metadata`) and writes `render-manifest.json`. Setup's screenshot command delegates to the same library implementation. This compatibility inventory retains historical header/dimension validation; accepted manifests and pixel comparison fully decode PNGs.

Validation finishes before acceptance writes begin. Writes are not a multi-file filesystem transaction: I/O failure during copying can require rerunning explicit acceptance after fixing disk permissions or capacity.
