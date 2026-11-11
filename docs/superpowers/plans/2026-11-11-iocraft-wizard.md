# iocraft wizard implementation plan

Spec: ../specs/2026-11-11-iocraft-wizard.md

- [x] Replace Cursive UI with channel-backed state and iocraft components while preserving public ui API. Write regressions for confirmation, defaults, Unicode editing, secret masking, menu/form/key paths and worker shutdown.
- [x] Adapt Windows mouse compatibility to iocraft TerminalBackend/event streams, preserving X10/SGR decoding and bounded Escape waits. Replay the previous bug against the new input path.
- [x] Replace Cargo dependencies, update maintainer documentation, verify no Cursive dependency/runtime remains and run complete locked Rust tests plus terminal render checks.
- [x] Review branch and resolve findings; prepare the branch for a PR with validation and publication requirements.

Validation: all 88 locked CLI tests pass, including 34 UI/input tests. Formatting and diff checks pass. The complete UI and Windows adapter type-check for x86_64-pc-windows-gnu in an isolated harness; the full CLI cross-check requires an unavailable MinGW C compiler for the existing ring dependency. Native Windows CI and a real IntelliJ terminal remain the integration checks. The published CLI and consumer pins remain unchanged until release.

Ownership: ui.rs + ui/{model,view}.rs are the UI owner; ui/{terminal,mouse_input}.rs are input compatibility owner. Root owns Cargo dependencies/lockfile, docs and integration. Iocraft0.9.5, smol2, futures0.3, unicode-segmentation1.13.3 are allowed. Existing answer/field/menu/key types and function signatures stay stable. Windows adapter exports ui::terminal::new() -> io::Result<WindowsTerminal> implementing iocraft::TerminalBackend. Rendering uses .render_loop().fullscreen().ignore_ctrl_c().enable_bracketed_paste(), attaching the adapter on Windows only. No unmasked secret TextInput.
