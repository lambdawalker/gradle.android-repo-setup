# Standalone source documentation

`android-repo-docs sources check` verifies reviewed Kotlin source hashes, API
excerpts and examples. It never writes. `sources update` explicitly refreshes
outputs after rendering and validating the complete configuration. The aggregate
`check` and `update` commands include sources when configured. An explicit
`sources check` without configuration fails.

## Configuration (schema 1)

Settings live in `.project/docs/config.yaml`. Sources are parsed independently
of publishing metadata: explicit source commands do not need Gradle properties,
release records, signing keys, setup binaries, or even the optional modules map.

```yaml
schema: 1
repository: owner/library
title: Library
tooling_ref: FULL_COMMIT_SHA
sources:
  roots: [library/src/main/kotlin, model/src/main/java]
  manifest: docs/source-manifest.json
  groups:
    - include: ['library/src/main/kotlin/**/*.kt']
      exclude: ['**/implementation/**']
      intro: .project/docs/introductions/core.md
      output: docs/api/core.md
    - include: ['model/**/*.kt']
      output: docs/guide.md
      start: '<!-- api:start -->'
      end: '<!-- api:end -->'
  examples:
    - source: app/src/main/java/example/MainActivity.kt
      target: docs/quickstart.md
      start: '<!-- quickstart:start -->'
      end: '<!-- quickstart:end -->'
    - source: src/main/java/example/Quickstart.kt
      target: docs/recipes.md
      start: '<!-- example:start -->'
      end: '<!-- example:end -->'
      region_start: '// docs:quickstart:start'
      region_end: '// docs:quickstart:end'
      prefix: |
        import example.Point
        import example.Distance
    - source: app/src/main/java/example/MainActivity.kt
      target: docs/handwritten.md
      start: '<!-- verified:start -->'
      end: '<!-- verified:end -->'
      mode: contains
```

Replace `FULL_COMMIT_SHA` with a full lowercase commit SHA containing
`android-repo-docs`; setup releases from before the standalone package cannot
install it.

All paths are repository-relative, slash-separated, nonempty paths without
traversal. Symlinks in roots, descendants, inputs, output paths or their existing
parents are rejected. Source roots must exist; they can be arbitrary directories,
including JVM `src/main/java`. Every `.kt` file beneath every root is parsed and
hashed, including files excluded from API groups. Overlapping roots deduplicate
files. Paths and records sort lexicographically.

| Setting | Required | Meaning |
| --- | --- | --- |
| `roots` | yes | Nonempty list of source directories, recursively inventoried |
| `manifest` | yes | Exact project-selected output JSON path |
| `groups` | no | Ordered group list; defaults to empty |
| `examples` | no | Example list; defaults to empty |
| group `include` | yes | List of globset globs matching full repository-relative paths |
| group `exclude` | no | Exclusion glob list; defaults to empty |
| group `intro` | no | UTF-8 Markdown file prepended to generated group content |
| group `output` | yes | Whole generated Markdown file, or existing marked-block target |
| group `start`, `end` | paired | Literal destination marker strings |
| example `source`, `target` | yes | Source text file and existing Markdown target |
| example `start`, `end` | yes | Literal destination markers |
| example `region_start`, `region_end` | paired | Literal source markers; omitted means full source |
| example `prefix` | no | Kotlin text prepended inside the generated code fence |
| example `mode` | no | Omitted means synchronize; `contains` verifies curated snippet |

Unknown source/group/example fields fail validation. First matching group owns a
file; later groups do not repeat it. Each file section has its path and a Kotlin
fence containing declaration excerpts. Intro text stays project-owned. No group
means inventory only. Omitted group destination markers mean replace the entire
output file. Both group markers mean replace only their interior, preserving
surrounding prose. Markers must be distinct, nonempty, uniquely present and in
order. Different nonoverlapping marked blocks can share a target. Duplicate or
overlapping scenarios and whole-file/marked-block collisions fail. Outputs cannot
overwrite source, intro, example-input or config files.

Synchronization examples wrap the entire source, or the text strictly between
source markers, in a Kotlin fence. Prefix imports are inserted before that text,
separated by a blank line. Source marker comments are omitted. Text outside target
markers is retained. `contains` requires the target block to contain exactly one
Kotlin fence; its nonempty contents must occur in the source after removing
whitespace from both. This mode preserves the target verbatim and rejects region
or prefix settings. Use it for curated snippets already present in compiled code.

The JSON manifest is a list of objects with exactly these generated fields:

```json
[
  {
    "path": "library/src/main/kotlin/example/Point.kt",
    "sha256": "64 lowercase hexadecimal digits",
    "guide": "docs/api/core.md",
    "visibility": "public"
  }
]
```

`sha256` covers the whole UTF-8 source after CRLF/lone-CR normalization to LF,
including implementation bodies and comments. Additions, deletions and
implementation-only edits cause check drift. `guide` is the first matching group's
output for a file with public excerpts, otherwise null. `visibility` is `public` when an excerpt remains, otherwise
`nonpublic`. Update writes LF text; checks accept Windows line endings. All
validation completes before the first output write; filesystem write failures
are reported but do not provide a multi-file transaction rollback.

## Kotlin excerpt boundaries

The engine uses locked `tree-sitter-kotlin-ng` 1.1.0 and tree-sitter 0.24.7.
Declarations retain overloads, annotations, type parameters, constructor defaults,
public nested structure, enum entry punctuation, property accessor visibility,
package/import context and relative source links. Files with no public excerpt
stay in the manifest with null guide and do not emit empty Kotlin fences. Private/internal/protected
declarations and their nested contents are omitted. Explicit constructor
visibility remains explicit. Function/accessor bodies and explicitly typed property initializers/delegates
are omitted. Inferred property initializers are retained verbatim to preserve
preset value/type context; inferred property types are not resolved. This is a source
excerpt, not ABI analysis or a compilable interface stub. Dependencies, inherited
visibility and symbol resolution are outside its scope.

Compatibility handling changes only a temporary parse view, with byte offsets
preserved: whitespace joining newline-before-catch and newline-before-safe-chain,
optional class-body separators in closing whitespace, and aliasing a bound local
soft-keyword identifier `open` outside strings/comments/modifiers. The grammar's
invisible missing `_class_member_semi` is accepted as Kotlin's optional final
separator. No general syntax errors are ignored. Remaining parse errors report
source path and original line and stop the operation. Hashes and excerpts always
use the original source text. No runtime Kotlin files are rewritten.

## Initialization

```sh
android-repo-docs init --repository owner/library --title 'Library' --tooling-ref FULL_COMMIT_SHA
android-repo-docs sources init --source-root custom/api --source-root custom/model
android-repo-docs sources update
android-repo-docs sources check
```

Global init accepts repeated `--source-root` too. Without explicit roots, init
uses saved selected library modules' remapped directories from setup config and
existing `src/main/java` / `src/main/kotlin` beneath them, plus root JVM source
directories. Applications are not inferred. Explicit source init fails if nothing
is found; global init can create metadata without source settings. The initial
manifest is `.project/docs/sources.json`, with empty groups/examples for project
review and customization. Existing sources settings are never replaced.

Fresh metadata uses explicit flags or GitHub git origin; title defaults to the
repository name. The tooling pin defaults to this docs package's release version,
not a copied old setup pin. Source builds should pass their full `--tooling-ref`.
Existing independent config is preserved. Installation examples also infer JVM
JAR publication from a confirmed record containing `.jar` and no `.aar` hash;
legacy records retain Android AAR defaults. Release records remain unchanged.
