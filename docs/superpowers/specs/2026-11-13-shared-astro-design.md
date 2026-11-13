# Shared Astro documentation backend

The agreed rollout is to audit the Android library sites (six current CLI consumers plus Signature adoption), implement the shared CLI first, let the maintainer publish its next version, then migrate consumers separately. No consumer repository is changed by this work.

The CLI release bundles a locked Astro/Starlight frontend. Rust owns immutable source/history/translation preparation, release evidence, scoped URLs, assets and raw Markdown. Astro owns presentation, search and interactive components. The same prepared content feeds Zola for compatibility. Bundling the frontend avoids requiring a separate npm publication when the maintainer releases the CLI.

New configurations default to Astro. Existing external command arrays remain external; existing sites/zola.toml remains an automatic Zola compatibility selection. Explicit site.backend values astro, zola, external and legacy override detection. Explicit built-in backends reject external command arrays. Legacy retains the old minimal static renderer. No site executes historical scripts, installs historical dependencies or evaluates archived MDX.

The shared frontend is staged in a temporary directory with pinned package.json/package-lock.json, prepared pages/scopes/site JSON, and immutable public assets. npm ci installs exact frontend dependencies; Node is a documented prerequisite and generated Actions select Node 24. Builds go to sites/dist, support base path overrides and --check link validation. Trusted current checkout overrides may live in sites/astro/components, styles and public. Canonical metadata takes precedence over override assets.

Existing deep links, language/module/version scopes, source/documentation provenance, stale translation fallback and raw guides are migration acceptance requirements. Consumer-specific route aliases and presentation changes must be assessed in the audit rather than silently declaring the renderer migrations complete.

Approaches considered: keep repository-local Astro copies (duplicated maintenance); publish a separate npm package (extra release dependency); bundle the reusable Astro package with the CLI (selected implementation choice, one coordinated release). Zola and external backends remain supported during the staged migration.
