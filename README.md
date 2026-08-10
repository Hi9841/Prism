# Prism



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
bun run tauri build
```

The NSIS installer is written to `src-tauri\target\release\bundle\nsis\`.

## Releases

Every push and pull request is verified by `.github/workflows/ci.yml`. After a merge reaches `main`, the same workflow builds a fresh NSIS installer after all checks pass. Download it from the **Artifacts** section of that run on the repository's Actions page.

A semantic version tag such as `v0.3.1` triggers `.github/workflows/release.yml`, which builds the NSIS installer, creates a permanent GitHub Release, and uploads the installer. Use tagged releases for versions intended for general distribution; per-merge workflow artifacts are development builds.

Before publishing, keep the version identical in `package.json`, `src-tauri\Cargo.toml`, `src-tauri\tauri.conf.json`, and `src-tauri\Cargo.lock`. Verify it with:

```powershell
.\scripts\check-version.ps1 -ExpectedTag v0.3.1
```

Publish a release:

```powershell
git tag v0.3.1
git push origin v0.3.1
```

The same workflow can be started manually from the repository's Actions page. It publishes the version currently declared by the application.

## Repository layout

- `src/` - React interface, state, and frontend tests
- `src-tauri/src/` - native Windows integration, indexing, and launcher behavior
- `src-tauri/icons/` - source and Windows bundle icons
- `src-tauri/nsis/` - installer artwork and lifecycle hooks
- `scripts/` - version checks, asset generation, and cleanup

Generated directories such as `dist/`, `src-tauri/gen/`, and `src-tauri/target/` are disposable. Remove them with:

```powershell
bun run clean
```
