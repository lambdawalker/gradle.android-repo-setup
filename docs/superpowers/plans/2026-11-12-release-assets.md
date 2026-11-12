# Reusable release assets

Implement the approved per-module asset contract for model files and other Android bundled assets.

- [x] Add tests for immutable Git/LFS manifests, scoped hydration, optional/empty sets, format validation, unsafe paths and archive verification.
- [x] Add `assets prepare --module` and a list of asset sets alongside legacy `required_assets`.
- [x] Integrate preparation before JitPack and generated release/verification builds; record resolved hashes and sizes and verify packaged/public assets.
- [x] Document migration and limitations; run the Rust suite and request a fresh review.

Decisions: the first release supports Git-backed AAR assets, glob `*`, `**`, `?`, and optional TFLite identification. It does not execute user-supplied validators or download arbitrary URLs. Existing `required_assets` remains accepted; mixing old/new forms is rejected to avoid ambiguous ownership. The Android consumer must keep its helper until the updated CLI is released.
