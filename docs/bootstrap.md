# Shared repository bootstrap

Set `.project/setup/config.yaml` `tooling_ref` to an existing stable `vMAJOR.MINOR.PATCH` release or a full lowercase Git commit SHA. Run `android-repo-setup bootstrap generate` to create `setup.sh`, `setup.bat`, `.project/setup/bootstrap.ps1`, and `.github/workflows/verify-setup.yml`. Existing modified files require `--overwrite`. Commit these generated files; regenerate them after changing the pin.

`sh ./setup.sh <arguments>` and `setup.bat <arguments>` forward arguments and the CLI exit status. They locate the repository relative to the launcher, regardless of the working directory. An omitted command retains interactive setup. `setup check` verifies bootstrap, workflows, publishing configuration, and evaluated Android module discovery without saving discovery results. Saved inventory is compared by Gradle path, kind, directory, namespace and application IDs. Legacy module selections are compared with evaluated libraries and the configured/default app; ambiguous configurations require `discover --save` and review.

On a desktop, run `sh ./setup.sh setup web` (Linux/macOS) or
`setup.bat setup web` (Windows) to open the same setup operations in a local
browser wizard. The CLI prints a manual local address if the browser cannot
open automatically; append `--no-open` after `web` to print the address without
opening a tab. Keep the launcher running while using the site. The browser
serves only on loopback and uses the same encrypted vault and repository
configuration as terminal setup; it does not change the generated
launcher or CI entry points. Keep its printed one-time link and session
recovery code private. If the session locks after 30 minutes without operator
activity, unlock it with the vault password or that recovery code. For the
workspace and credential behavior, see the
[browser setup wizard](../README.md#browser-setup-wizard).

Release pins download the matching Linux x86_64, macOS arm64, or Windows x86_64 archive and its SHA256 sidecar from the pinned GitHub release. Checksum and CLI version verification finish in a temporary directory before installation into `.project/setup/cache/<pin>`. Unsupported platforms report how to use a full SHA for a Cargo source build. Commit pins run `cargo install --locked --rev` with an isolated installation stage and cache the resulting executable by pin. Rust and Git are required for this source path. Cargo compiles independent dependencies in parallel by default. Source builds keep intermediate artifacts for each pin in `~/.cache/android-repo-setup/cargo-target/<pin>` on Linux/macOS or `android-repo-setup/cargo-target/<pin>` under Windows Local AppData. Repositories using the same pin can reuse those artifacts; a new pin gets a separate target to prevent Cargo from reusing the previous revision's executable. The first source build for a pin still compiles its dependencies. These per-user build caches can be removed when no install is running to recover disk space. Cached release executables are version checked before use; remove a broken project cache directory to reinstall. Checksums authenticate archive integrity against the release sidecar; they do not replace trust in the GitHub release publisher.

`bootstrap check` compares generated content without modifying files. Generation rejects symlink destinations. Add `.project/setup/cache/` to the repository ignore rules. Enable optional JitPack generation with:

```yaml
bootstrap:
  jitpack: true
```

The generated JitPack install command invokes `sh ./setup.sh jitpack-build`. Existing project JDK and `before_install` settings are preserved. The setup workflow verifies the complete setup on Linux and smoke tests the Windows launcher and bootstrap content on Windows.
