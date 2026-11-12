# Shared bootstrap and consumer cleanup

Goal: centralize reusable helpers in the Rust CLI; leave at most two project-specific executable scripts per configured Android consumer. User approved shared bootstrap/setup-check direction and requested all consumers.

Architecture: CLI owns generated root launchers plus one generated PowerShell runtime under .project/setup, declarative screenshot/test inventories, isolated release validation, and setup verification. Keep Card Detector's domain-specific Kotlin documentation prose/extraction and historical-media policy as its two scripts. No weakened verification or historical metadata rewrites.

Consumers verified: carddetector, demo-common, permissions. Auth/yolo/math.coordinates/signature have no CLI config; do not onboard unrelated repos.

- [x] Bootstrap generation/check, platform-specific pinned installers, safe cached validation, SHA Cargo fallback, generated setup workflow and JitPack config.
- [x] setup check including evaluated module/config drift; retain interactive setup.
- [x] Generic screenshot manifest and required JUnit-test checks; move consumer data to .project/verification.
- [x] Generic isolated release-validation command driven by configured argv steps; preserve Permissions temporary worktree isolation.
- [x] Migrate three consumers, retain two custom Card Detector scripts, remove obsolete helpers/dependencies/tests with coverage relocated to CLI.
- [x] Full CLI checks, consumer checks, fresh review, push CLI + dependent consumer PRs using exact CLI source SHA until release.

Release strategy: do not invent a release tag. Consumer PRs pin the pushed CLI commit and use existing released Gradle plugins. No publishing/merging without user direction.
