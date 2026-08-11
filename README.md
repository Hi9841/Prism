# Prism

[Demo attachment](https://github.com/user-attachments/assets/ec4ac19a-8868-4a01-b9f4-10008c297b6c)


Prism is a lightweight, keyboard-first launcher for Windows. It provides fast local application, file, and folder search, plus quick access to frequently used locations.

## Download

Download the latest `Prism_*_x64-setup.exe` from [GitHub Releases](https://github.com/Hi9841/Prism/releases/latest).

Prism installs for the current Windows user without administrator access. The installer downloads the WebView2 bootstrapper only when the runtime is missing. Builds are currently unsigned, so Windows SmartScreen may display a warning.

The installer registers Prism as a per-user startup app. It starts hidden at Windows sign-in so the global shortcut is ready immediately, and it can be managed from **Task Manager > Startup apps**.

## Development

Requirements:

- Windows 10 or Windows 11 on x64
- [Bun](https://bun.sh/) 1.3.14
- Rust stable with the MSVC toolchain, Clippy, and rustfmt
- Visual Studio Build Tools with the Desktop development with C++ workload

Install dependencies and run Prism:

```powershell
bun install --frozen-lockfile
bun run tauri dev
```

Run all local checks:

```powershell
bun run lint
bun run test
bun run build
.\scripts\check-version.ps1
cargo fmt --manifest-path src-tauri\Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri\Cargo.toml
```

Build the Windows installer:

```powershell
.\scripts\generate-installer-assets.ps1
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.prism\signing\updater.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = Get-Content -Raw "$env:USERPROFILE\.prism\signing\updater.key.password"
bun run tauri build
```

The NSIS installer is written to `src-tauri\target\release\bundle\nsis\`.

## Releases

Release files are built and published locally. Updater-enabled Prism builds check the latest GitHub Release at startup and while the app is running, then offer a signed release from the launcher footer.

Before publishing, keep the version identical in `package.json`, `src-tauri\Cargo.toml`, `src-tauri\tauri.conf.json`, and `src-tauri\Cargo.lock`. Verify it with:

```powershell
.\scripts\check-version.ps1 -ExpectedTag v0.6.4
```

Build the signed installer, place the installer and its `.sig` file in `artifacts`, and add `artifacts\Prism_<version>_release-notes.md`. Push the matching tag, then publish:

```powershell
git tag 0.6.4
git push origin 0.6.4
.\scripts\publish-release.ps1
```

`publish-release.ps1` accepts either `0.6.4` or `v0.6.4` tags. It creates or repairs the GitHub Release, uploads the installer, signature, release notes through the release body, and `latest.json`, then verifies the public updater endpoint. Use `-Tag v0.6.4` when publishing a `v`-prefixed tag.

The updater signing private key and password are stored outside this repository under `%USERPROFILE%\.prism\signing\`. Back up both files securely: existing installations reject updates signed by a replacement key. Only the public key is committed in `src-tauri/tauri.conf.json`.

The first release containing the updater still requires a normal installer upgrade for users on older Prism builds. After that one-time transition, newer tagged releases can be installed from the footer update control.

## Repository layout

- `src/` - React interface, state, and frontend tests
- `src-tauri/src/` - native Windows integration, indexing, and launcher behavior
- `src-tauri/icons/` - source and Windows bundle icons
- `src-tauri/nsis/` - installer artwork and lifecycle hooks
- `scripts/` - version checks, asset generation, release publishing, and cleanup

Generated directories such as `dist/`, `src-tauri/gen/`, and `src-tauri/target/` are disposable. Remove them with:

```powershell
bun run clean
```
