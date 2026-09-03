# Release guide

This guide is for maintainers preparing source and desktop releases.

The repository also provides **Actions → Build desktop release → Run workflow** for a complete automated release from `main`. It is manual-only: pushes, pull requests, and tags do not trigger a build. The workflow generates a version in `yy.MM.dd.HHmm` format using the Asia/Ho_Chi_Minh time zone, builds Windows x64/x86 and macOS Intel/Apple Silicon packages, publishes checksums, and marks the new GitHub release as latest.

## 1. Prepare the release

1. Start from reviewed stable history and ensure `dev` contains the intended changes.
2. Decide the semantic version.
3. Update versions consistently in:
   - root `Cargo.toml`;
   - each workspace crate `Cargo.toml`;
   - `web/package.json` and `web/package-lock.json`;
   - `chatgpt-extension/manifest.json` when the extension changed.
4. Update [CHANGELOG.md](../CHANGELOG.md): move relevant entries from **Unreleased** into a dated version section.
5. Verify user, plugin, security, migration, and protocol documentation.
6. Confirm no secret, local database, log, source map, personal path, or unsigned private credential is staged.

`CHATCMD_BUILD_VERSION` overrides the displayed/package version without changing source metadata. Public releases should still keep source manifests aligned.

## 2. Run quality gates

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

cd web
npm ci
npm run lint
npm test -- --run
npm run build

cd ../chatgpt-extension
node --test content-chatgpt.test.cjs
```

Inspect dependency and license changes. Perform manual smoke tests for access-profile creation, endpoint rotation, MCP connection, permissions, approvals, terminal lifecycle, task completion, data persistence/restart, and extension behavior.

## 3. Build Windows packages

Run from a Windows developer environment with Rustup and both MSVC targets available:

```powershell
.\scripts\build-windows.ps1 -Version 0.1.0
```

The script:

- installs/checks `x86_64-pc-windows-msvc` and `i686-pc-windows-msvc`;
- builds and obfuscates the frontend without source maps;
- builds `embedded-web` release binaries;
- copies and obfuscates the unpacked extension;
- creates 64-bit and 32-bit directories and ZIP archives under `release/`.

Verify both architectures on clean supported systems. Confirm `ChatCMD.exe` metadata and icon, browser launch, tray exit, database creation, and extension contents.

## 4. Build macOS packages

Run on macOS with Rustup, Xcode command-line tools, and the required signing identity:

```bash
CHATCMD_BUILD_VERSION=0.1.0 \
MACOS_SIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
MACOS_NOTARY_PROFILE="ChatCMDNotary" \
./scripts/build-macos.sh
```

If no signing identity is found, the script can produce an ad-hoc signed development package. Do not present an ad-hoc package as a notarized public release.

The script builds Apple Silicon and Intel targets, embeds the frontend, creates app bundles and icons, includes the extension, signs the app, optionally notarizes/staples it, and creates ZIP archives under `release/`.

Test both architectures or document any untested artifact. Verify Gatekeeper behavior, tray lifecycle, browser launch, database location, and extension loading.

## 5. Inspect artifacts

- Ensure ZIP names and displayed versions match the release.
- Confirm no `.map` files exist.
- Confirm source-only files, databases, logs, and signing material are absent.
- Scan archives with the available platform security tools.
- Generate and publish SHA-256 checksums.
- Extract each archive into a clean directory and smoke-test the extracted copy.
- Confirm the bundled extension manifest version and source match the release notes.

## 6. Tag and publish

1. Merge the reviewed release state into `main`.
2. Create a signed or annotated tag such as `v0.1.0` from the release commit.
3. Push the branch and tag.
4. Create a GitHub release using the matching changelog section.
5. Attach platform archives and checksums.
6. Clearly label prereleases, unsigned artifacts, unsupported platforms, migrations, breaking changes, and known limitations.
7. Keep source archives available under the MIT license.

## 7. After release

- Verify download links and checksums from another machine.
- Confirm the version displayed by the app.
- Restore an **Unreleased** section in the changelog if needed.
- Announce security-relevant changes through the coordinated disclosure agreed with reporters.
- Merge urgent release-only fixes back into `dev` so branches do not drift.
