# iocraft wizard migration

Replace Cursive with published iocraft 0.9.5 in the shared Rust CLI. Keep the external ui API and all setup, publishing and vault code semantics. The new full-screen UI uses a responsive header, question/form panel, context/progress area, status and keyboard hints. Native iocraft buttons provide mouse hit testing for actions and styled checkbox controls; application state owns keyboard focus and editing. Secrets are never passed to a normal unmasked TextInput: a custom single-line buffer produces masked display text while preserving Unicode editing and paste-at-cursor.

Preserve non-secret 20-second default/reuse countdowns that pause on actual interaction; secrets and confirmations have no automatic approval. Apply requires an explicit checked confirmation. Menu defaults select all only on first run; all operations, help, vault view/reset, forms/validation, saved-value choices and signing-key loading remain available. Async setup/GPG/network work stays off the render loop; stale key results are ignored and UI termination unblocks outstanding requests.

Iocraft uses Crossterm too. Adapt the already-tested Windows X10/SGR mouse-report normalization before iocraft dispatch; do not regress IntelliJ input or disable mouse support. Keep native keys/events and non-Windows behavior. No signing material or tokens enter logs, snapshots or test diagnostics. Existing vault ciphertext formats and release metadata stay unchanged.

Use released crates with a lockfile. Validate state transitions, masked rendering, Unicode/clipboard/focus, key loading cancellation, fragmented mouse streams, terminal cleanup and current non-UI tests. Ship one reviewable CLI PR; consumer pins change only after a compatible CLI release is published.
