# Consumer documentation migration readiness

Audited 2026-11-13 against GitHub `main`, using the connected GitHub API for branch heads, recursive trees and files pinned to those heads. Local consumer checkouts were not treated as current evidence. This is a source/configuration audit, not a claim that any Pages deployment is live or that a consumer has passed an Astro migration build. No consumer repository was changed.

## Current state

The six existing shared-CLI consumers already pin the documentation CLI to **v0.1.20** in `.project/docs/config.yaml` and their documentation workflows. That version update is complete; it did **not** migrate all renderers or consolidate the existing Astro sites into the shared pipeline. A seventh repository, Signature, has an existing custom website but has not adopted the shared CLI.

| Repository | Audited main commit | Active renderer at that commit | Actual migration remaining |
| --- | --- | --- | --- |
| `android.apexfission.carddetector` | [`116c4d3679497c888d8bb46faf3755bfa1ee17df`](https://github.com/lambdawalker/android.apexfission.carddetector/commit/116c4d3679497c888d8bb46faf3755bfa1ee17df) | Consumer-owned Astro 7.3.5 / Starlight 0.42.4 through `site.build` | Consolidate pipeline, preserve two-module behavior and media presentation |
| `android.apexfission.permissions` | [`e5dc523e705bd2b7336f8f4175068ab308aa9761`](https://github.com/lambdawalker/android.apexfission.permissions/commit/e5dc523e705bd2b7336f8f4175068ab308aa9761) | Shared CLI default Zola 0.23.6; no `site.build` override | Real renderer change plus replacement of local Tera overrides and navigation tests |
| `android.apexfission.demo-common` | [`52ef204657ec3b829e98a803253f4c3b2d6092b2`](https://github.com/lambdawalker/android.apexfission.demo-common/commit/52ef204657ec3b829e98a803253f4c3b2d6092b2) | Shared CLI default Zola 0.23.6; no `site.build` override | Real renderer change, preserving verified provider equivalence and corrected archives |
| `android.apexfission.auth` | [`34683fca07ba677e26577f0d96ca1f7f3b35cd62`](https://github.com/lambdawalker/android.apexfission.auth/commit/34683fca07ba677e26577f0d96ca1f7f3b35cd62) | Consumer-owned Python-Markdown through `site.build: [python, sites/build.py]` | Real renderer change, route/identity and translation-manifest adaptation |
| `android.apexfission.yolo` | [`288e06dfcda248986e16a3a74d430f91edf591ba`](https://github.com/lambdawalker/android.apexfission.yolo/commit/288e06dfcda248986e16a3a74d430f91edf591ba) | Consumer-owned Astro 7.3.5 / Starlight 0.42.4 through `site.build` | Consolidate duplicate rendering and retain archive correction notices |
| `android.apexfission.math.coordinates` | [`5edccfbf1e5e5a85b3a579514d6ad04efc3506dc`](https://github.com/lambdawalker/android.apexfission.math.coordinates/commit/5edccfbf1e5e5a85b3a579514d6ad04efc3506dc) | Consumer-owned Astro 7.3.5 / Starlight 0.42.4 through `site.build` | Consolidate duplicate rendering and preserve executable-documentation verification |
| `android.apexfission.signature` | [`c2c3182016a527ed4a03a1f1d51d294f3b8f36f3`](https://github.com/lambdawalker/android.apexfission.signature/commit/c2c3182016a527ed4a03a1f1d51d294f3b8f36f3) | Consumer-owned Python-Markdown via `scripts/docs.py`; no shared CLI configuration | First shared CLI adoption plus migration of an existing renderer; not first website creation |

Changing only `tooling_ref` or the workflow install tag leaves all four explicit `site.build` overrides active. Removing an override is a separate migration decision and requires proving equivalent output. Permissions and Demo Common retain Zola through compatibility autodetection while `sites/zola.toml` remains and no explicit Astro backend is selected. A release bump alone therefore does not switch either renderer: migration requires explicitly selecting Astro or removing the Zola configuration as appropriate. Their Zola-only overrides do not become Astro components automatically.

## Ownership and preservation work

### Card Detector

[Configuration](https://github.com/lambdawalker/android.apexfission.carddetector/blob/116c4d3679497c888d8bb46faf3755bfa1ee17df/.project/docs/config.yaml), [site package](https://github.com/lambdawalker/android.apexfission.carddetector/blob/116c4d3679497c888d8bb46faf3755bfa1ee17df/sites/package.json), [version builder](https://github.com/lambdawalker/android.apexfission.carddetector/blob/116c4d3679497c888d8bb46faf3755bfa1ee17df/sites/scripts/build-versioned.mjs).

`sites/scripts/{sync,markdown,navigation,versioned,build-versioned,validate-docs}.mjs` and four `Version*`/language components implement local staging, rewriting, snapshots, selectors and validation. These are candidates for shared ownership. `sites/src/styles/brand.css`, `showcase.mjs` and `model-pin.mjs` carry consumer requirements and must be retained or represented explicitly in shared configuration/hooks.

The detector and `tfmodel` have independent release histories. The model displays a restricted guide set, model-contract pages and the exact detector dependency pin from confirmed records. Quickstart links and model prerequisites have special routing. A generic all-guides-for-all-modules render would change this behavior. The showcase adds scoped overview/project-resource pages and clearly labels current synthetic screenshots as illustrations rather than release verification. Preserve that distinction and provenance, not just the images.

Media includes an LFS-backed demo video, an extracted historical tracking frame, reviewed UI images and bilingual capture sequences. Keep `media` manifest/source/dimension validation, full-history checkout and `lfs: true`. Preserve `/screenshots/` URLs and root agent/installation/discovery endpoints. CI currently installs Node 24 and runs local npm tests/build through the override; migrate to shared Astro installation/build only after project-specific model/media checks have an explicit home.

### Permissions

[Site documentation](https://github.com/lambdawalker/android.apexfission.permissions/blob/e5dc523e705bd2b7336f8f4175068ab308aa9761/sites/README.md), [landing tests](https://github.com/lambdawalker/android.apexfission.permissions/blob/e5dc523e705bd2b7336f8f4175068ab308aa9761/sites/tests/landing.sh), [workflow](https://github.com/lambdawalker/android.apexfission.permissions/blob/e5dc523e705bd2b7336f8f4175068ab308aa9761/.github/workflows/documentation.yml).

Local `sites/templates/{index,page,release_availability}.html` are Tera overrides for direct latest-release entry, exact-patch selection and provider availability. `sites/static/release-docs.css` styles them. Port their required behavior to shared Astro; copying these templates into Astro has no effect. `sites/package.json` is a CLI/test wrapper, not an Astro site.

Keep module ID `permission` (singular), `/en/permission/<version>/` and `/es/permission/<version>/`, scoped raw Markdown, legacy human redirects in `sites/static/*/index.html`, older version aliases and root machine-readable endpoints. Historical Central 0.2.1 intentionally lacks a source manual; do not invent a snapshot. Current history files cover 0.2.3–0.2.6.

`docs/screenshots/manifest.json` owns reviewed images; `sites/static/screenshots` intentionally preserves legacy URLs and drift is checked. Screenshot candidate rendering/acceptance must remain separate from the website renderer. Replace the Zola fixture setup and `sites/dist/docs.js`-specific navigation regressions with behavior checks for the Astro output. Workflow currently installs checksum-pinned Zola and runs `landing.sh`, then screenshot-manifest checks; update those prerequisites while preserving full Git history, LFS, trusted release triggers and stale-main deployment guard.

### Demo Common

[Site documentation](https://github.com/lambdawalker/android.apexfission.demo-common/blob/52ef204657ec3b829e98a803253f4c3b2d6092b2/sites/README.md), [equivalence evidence](https://github.com/lambdawalker/android.apexfission.demo-common/blob/52ef204657ec3b829e98a803253f4c3b2d6092b2/sites/static/release-equivalence.json), [template regressions](https://github.com/lambdawalker/android.apexfission.demo-common/blob/52ef204657ec3b829e98a803253f4c3b2d6092b2/sites/tests/landing.sh).

Uses the same three local Tera overrides and availability CSS as Permissions. There is no local npm renderer. Port direct latest-release entry, numeric version comparison, exact-patch options, separate Development, and guide-preserving selectors to shared Astro.

Historical 0.1.2 Central and JitPack publications have different source commits. Their equivalence is explicitly backed by matching AAR, sources and Javadoc hashes in `release-equivalence.json`; equal version text alone is insufficient. Preserve both provider-specific routes/records while displaying a single verified-equivalent version option. Preserve source commit, reviewed `documentation_ref`, optional retained translation tree and current documentation-build identity separately.

Corrected bilingual 0.1.2 documentation and release-specific catalogue screenshots are retained through immutable history, with a supporting `docs/demo-common-0.1.2-catalog` branch documented for reachability. Full-history checkout must obtain those objects; never silently replace old screenshots with current development UI. Retain manifest dimension checks, root raw endpoints and legacy `/<version>/<language>/*.html` aliases. The workflow's checksum-pinned Zola install and Zola-only template fixtures require replacement; keep Pages triggers and stale-main guard.

### Auth

[Renderer configuration](https://github.com/lambdawalker/android.apexfission.auth/blob/34683fca07ba677e26577f0d96ca1f7f3b35cd62/sites/config.json), [builder](https://github.com/lambdawalker/android.apexfission.auth/blob/34683fca07ba677e26577f0d96ca1f7f3b35cd62/sites/build.py), [translation manifest](https://github.com/lambdawalker/android.apexfission.auth/blob/34683fca07ba677e26577f0d96ca1f7f3b35cd62/sites/translations.json), [hosting contract](https://github.com/lambdawalker/android.apexfission.auth/blob/34683fca07ba677e26577f0d96ca1f7f3b35cd62/sites/README.md).

The local Python renderer, CSS, browser script and tests own the entire site. Its source glob includes top-level Markdown, operational `docs/*.md`, `docs/identity/*.md` and `sites/README.md`, beyond consumer `docs/agents`. Preserve or deliberately relocate that broader navigation before deleting the override.

There is an explicit identity discrepancy: shared docs config names `lambdawalker/android.apexfission.auth`, but `sites/config.json` still names `lambdawalker/android.attestra.auth`; the builder derives `/android.attestra.auth/`, canonical links and source links from the latter. Resolve intended public identity and preserve any required old paths during the later migration. This audit does not infer whether the old URL is live. Deployment is gated on `DOCS_PAGES_ENABLED == 'true'`; preserve that gate.

Existing human routes are `/en/main/<source-path>.html` and `/es/main/<source-path>.html`, with raw routes `/raw/<language>/main/<source-path>.md`, root `/agents/index.md`, `IMPORT.md`, `llms.txt` and `versions.json`. A shared `/en/auth/development/` scheme needs compatibility redirects/raw aliases. The catalog advertises Development only and explicitly rejects release snapshots; do not fabricate published history while migrating.

Translations use `sites/translations.json`, keyed by full canonical paths with translated path, source SHA-256 and review provenance; this differs from the other libraries' `docs/es/translations.json` shape. Preserve code-fence checks, same-revision fallback and provenance rather than refreshing hashes mechanically. Curated screenshot manifests include image hashes; retain the independent screenshot-suite coverage and verification checks. Remove Python-Markdown/Pygments renderer dependencies only after porting their site checks; screenshot tooling may still require Python. Keep Windows/POSIX launcher verification and stale-main deployment guard.

### YOLO and Coordinates

[YOLO version builder](https://github.com/lambdawalker/android.apexfission.yolo/blob/288e06dfcda248986e16a3a74d430f91edf591ba/sites/scripts/build-versioned.mjs), [Coordinates version builder](https://github.com/lambdawalker/android.apexfission.math.coordinates/blob/5edccfbf1e5e5a85b3a579514d6ad04efc3506dc/sites/scripts/build-versioned.mjs), [Coordinates workflow](https://github.com/lambdawalker/android.apexfission.math.coordinates/blob/5edccfbf1e5e5a85b3a579514d6ad04efc3506dc/.github/workflows/deploy-library-site.yml).

These are the simplest existing Astro consolidation candidates: one module each (`yolo`, `coordinates`), same exact Astro/Starlight pins, local Markdown rewriting, archive generation, validation, selector components and brand CSS. YOLO starts from `sync.mjs`; Coordinates uses `sync-docs.mjs` and also retains `sync-versioned.mjs`. Inventory references before deleting apparently duplicate scripts.

Both consume shared `history export`, then independently render immutable `documentation_ref` snapshots, optional translation trees, confirmed per-destination installation text and bilingual development scopes. Both add a postrelease correction notice explaining that runnable examples were added at the documentation revision and must be run there. Preserve that source/examples distinction and English anchor compatibility in Spanish pages.

Keep `/en/<module>/<version>/`, `/es/<module>/<version>/`, matching `raw/*.md`, legacy installation paths, root `agents/*.md`, `IMPORT.md`, `llms.txt`, catalog and source links. Coordinates has public SVG assets (`crop.svg`, favicon) and Markdown/link tests; YOLO generates its favicon and root discovery files. Coordinates explicitly has no screenshot prerequisite. Coordinates CI additionally runs `./gradlew test runExamples verifyImportDocs` before site rendering; this is executable-documentation verification and remains consumer-owned. YOLO uses Node 24; Coordinates workflow currently uses Node 22. Ensure the chosen shared Astro runtime is supported in both before changing npm ownership.

### Signature: first shared CLI adoption, existing website

[Site contract](https://github.com/lambdawalker/android.apexfission.signature/blob/c2c3182016a527ed4a03a1f1d51d294f3b8f36f3/sites/README.md), [renderer](https://github.com/lambdawalker/android.apexfission.signature/blob/c2c3182016a527ed4a03a1f1d51d294f3b8f36f3/scripts/docs.py), [validation workflow](https://github.com/lambdawalker/android.apexfission.signature/blob/c2c3182016a527ed4a03a1f1d51d294f3b8f36f3/.github/workflows/validate.yml).

Fresh main contains an existing bilingual documentation website, despite having no `.project/docs/config.yaml`, shared docs launcher, Astro config or Zola config. `scripts/docs.py` builds ten English and ten Spanish HTML guides plus raw Markdown, inline-generated styling, `llms.txt` and `catalog.json`; `sites/requirements.txt` pins its Python dependencies. CI runs Python tests and `scripts/docs.py verify`, then uploads `sites/dist` as `development-human-and-agent-docs`. It does not deploy Pages or publish packages. Future shared CLI adoption must not silently add deployment.

The root opens visibly labeled Development. Existing human URLs are `/<language>/signature/development/<guide>.html`; raw URLs are `/raw/<language>/signature/development/<guide>.md`. Links are relative so the artifact can be hosted under a project subpath. Preserve aliases if adopting shared trailing-slash routes and scoped raw layout. `docs/releases.json` explicitly has no confirmed releases, and the renderer rejects adding historical releases until immutable rendering exists. Do not invent publication history.

Canonical guides live in `docs/agents`, Spanish in `docs/es`, while freshness hashes live in `docs/translations.json`, not `docs/es/translations.json`. `docs/source-manifest.json` covers source inputs from both `protocol` and `signature` modules and the compiled integration example. Port intentional source/translation acceptance, same-development fallback, code-fence equality and link checks before retiring the local verifier; do not turn the existing reviewed input-hash contract into unchecked regeneration. The current footer source link points to `feat/sworn-ink-signatures`; shared rendering should establish exact build/source provenance rather than copying that mutable branch link. This headless library owns no UI screenshots; do not add screenshot prerequisites.

This is a real migration from an existing custom renderer coupled with first-time shared CLI configuration, not an omitted migration caused by having no website. It is a useful Development-only adoption case once configuration/source-manifest compatibility is defined; it should follow the common single-module Astro pilots and can precede Auth's broader route/identity work.

## Shared pipeline requirements and rollout boundary

Before adopting a released shared Astro default in a consumer:

1. Install/pin the new CLI and its supported Node/runtime dependencies; update launchers, workflow tags and setup-generated pins together. A version bump alone is not proof of renderer consolidation.
2. Keep canonical sources and release metadata consumer-owned. Move generic staging, safe Markdown rendering/link rewriting, archive catalogs, selectors, search and validation into shared code. Expose explicit consumer presentation/asset configuration where needed rather than duplicating a full pipeline.
3. Remove `site.build`/`site.serve` overrides only when shared output satisfies that consumer's requirements. Keep a documented explicit override escape hatch for intentional custom sites. Keep legacy Zola opt-in available while Tera-dependent consumers are deferred.
4. Compare generated route sets and raw endpoints before/after, including base paths, English/Spanish anchors, legacy aliases, scoped search and guide retention across module/version/language changes. Verify root redirects select the numerically latest confirmed release where currently required.
5. Build every archive from a clean output directory with full history. Fail on missing historical commits/translation trees. Read historical Markdown/assets as data; never execute historical scripts. Distinguish code source, documentation correction, translation tree and current builder revision in output.
6. Preserve confirmed destinations, archive boundaries, independent module versions, equivalence evidence and screenshot provenance. Do not mint releases or alter historical publication facts to make rendering pass.
7. Retain PR read-only builds, successful trusted-main release triggers, source-SHA stale-deployment guards and Pages permissions/gates. Retry docs without republishing Android artifacts.

Suggested later order: **Coordinates → YOLO → Permissions → Demo Common → Card Detector → Signature → Auth**. Coordinates and YOLO establish parity for the common local Astro implementation; Permissions exercises the actual Zola-to-Astro transition and aliases; Demo Common adds evidence-based provider grouping; Card Detector adds independent model behavior and substantial media; Signature adds first-time shared configuration and source-manifest adaptation; Auth needs the distinct public-path/identity, catalog and translation decisions. This ordering is a risk assessment, not authorization to edit or deploy consumers.

The present change is limited to preparing the shared CLI and this audit. Publishing the CLI is the user's next step; consumer migrations remain deferred until the released version can be adopted and checked explicitly.
