# Shared repository bootstrap

Set `.project/setup/config.yaml` `tooling_ref` to an existing stable `vMAJOR.MINOR.PATCH` release or a full lowercase Git commit SHA. Run `android-repo-setup bootstrap generate` to create `setup.sh`, `setup.bat`, `.project/setup/bootstrap.ps1`, and `.github/workflows/verify-setup.yml`. Existing modified files require `--overwrite`. Commit these generated files; regenerate them after changing the pin.

`sh ./setup.sh <arguments>` and `setup.bat <arguments>` forward arguments and the CLI exit status. They locate the repository relative to the launcher, regardless of the working directory. An omitted command retains interactive setup. `setup check` verifies bootstrap, workflows, publishing configuration, and evaluated Android module discovery without saving discovery results. Saved inventory is compared by Gradle path, kind, directory, namespace and application IDs. Legacy module selections are compared with evaluated libraries and the configured/default app; ambiguous configurations require `discover --save` and review.

Release pins download the matching Linux x86_64, macOS arm64, or Windows x86_64 archive and its SHA256 sidecar from the pinned GitHub release. Checksum and CLI version verification finish in a temporary directory before installation into `.project/setup/cache/<pin>`. Unsupported platforms report how to use a full SHA for a Cargo source build. Commit pins run `cargo install --locked --rev` in an isolated temporary target directory and cache the result. Rust and Git are required for this source path. Cached release executables are version checked before use; remove a broken cache directory to reinstall. Checksums authenticate archive integrity against the release sidecar; they do not replace trust in the GitHub release publisher.

`bootstrap check` compares generated content without modifying files. Generation rejects symlink destinations. Add `.project/setup/cache/` to the repository ignore rules. Enable optional JitPack generation with:

```yaml
bootstrap:
  jitpack: true
```

The generated JitPack install command invokes `sh ./setup.sh jitpack-build`. Existing project JDK and `before_install` settings are preserved. The setup workflow verifies the complete setup on Linux and smoke tests the Windows launcher and bootstrap content on Windows.
