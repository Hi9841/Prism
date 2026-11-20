# Prism

[![Latest release](https://img.shields.io/github/v/release/Hi9841/Prism)](https://github.com/Hi9841/Prism/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform: Windows 10 | 11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4)

Prism is a keyboard-first launcher for Windows. Use it to find apps, files, folders, and Windows tools.
On Windows 11, you can also customize the taskbar. Prism uses Rust and [GPUI](https://www.gpui.rs/).

## Features

- Search installed desktop apps and packaged (Microsoft Store) apps.
- Search Windows tools, Control Panel applets, and installed power plans.
- Find local files and folders by name or path.
- Pin and reorder apps, and keep up to six Quick Access folders.
- Open Prism with a global shortcut or the Windows Start button.
- Run eligible apps and scripts as administrator from the context menu.
- Follow the Windows theme or select the light or dark theme. Set the accent and background colors
  with presets or the color picker.

## Install

Download the latest `Prism_<version>_x64-setup.exe` from
[GitHub Releases](https://github.com/Hi9841/Prism/releases/latest).

- Windows 10 or Windows 11, x64.
- Install Prism for the current user. The installer requires no administrator access.

SmartScreen can show a warning because the installer has no Authenticode signature. Prism verifies
in-app updates with its minisign public key.

## Usage

1. Open Prism with the Windows key or your shortcut.
2. Type an app name, a file name, a folder path, a Windows tool, or a calculation.
3. Use the arrow keys to select a result. Press Enter.

Prism starts in the background when you sign in to Windows. To change this, open **Task Manager > Startup apps**.

### Run as administrator

Right-click an eligible app or script and select **Run as administrator**. `Shift+F10` and the Menu key
open the same menu. Windows shows its usual UAC prompt before the target starts.

| Type | Extensions |
|---|---|
| Applications | `.exe`, `.com` |
| Batch scripts | `.bat`, `.cmd` |
| PowerShell scripts | `.ps1` |
| Windows Script Host | `.vbs`, `.js`, `.wsf` |

Microsoft Store apps start through Windows app activation, so they do not show this action.

### Taskbar customization

- Align the taskbar apps to the left or to the center.
- If you use the StartAllBack classic taskbar, you can also align the apps to the right.
- Select Compact, Default, or When full icon density.
- Set auto-hide and choose when to combine taskbar buttons.
- Select the System, Gem, or Diamond Start button icon, or add your own PNG (`96 x 96`, transparent
  background).

Windows 11 controls the height of the taskbar. Taskbar density changes the icon scale only.

### Updates

When Prism finds an update, use the update control in the launcher footer to install it.

## Build from source

### Requirements

- Windows 10 or Windows 11, x64
- Rust stable with the MSVC toolchain, Clippy, and rustfmt
- Visual Studio Build Tools with the **Desktop development with C++** workload
- [Bun](https://bun.sh/) 1.3.14 for the behavior judge and installer scripts

### Build and run

```powershell
cargo run -p prism-app
```

### Check the workspace and launcher behavior

```powershell
cargo check --workspace --all-targets
cargo build -p prism-judge
bun migration/judge/judge.mjs
```

`PRISM_ADAPTER_CMD=<exe>` runs the judge against a different adapter.

## Project structure

```
crates/
  prism-app/      prism.exe: window, global hotkey, tray, updater host
  prism-ui/       views and state
  prism-core/     backend: Windows integration, file index, search, taskbar, audio
  prism-judge/    JSON adapter for the behavior judge
  prism-vocab/    shared settings values and app source names
vendor/gpui/      GPUI 0.2.2 with Prism's patches (PRISM-PATCHES.md)
packaging/        installer build, NSIS script, version check, release publisher
migration/        behavior judge and rulebook
oploop/           performance ledger, benchmarks, functional test of the real app
```

## Release

To publish a release, increase the workspace version in `Cargo.toml` and run
`cargo update --workspace`. Include both changes in the PR. When the repository owner
approves the final PR commit and merges it into `main`, GitHub Actions tags the merged commit.
It publishes the Windows installer, updater signature, and updater manifest.

### Manual release

1. Set the version in `Cargo.toml` (`[workspace.package]`), then run `cargo update --workspace` to update
   `Cargo.lock`.
2. Build the installer. The script writes the exe, the NSIS installer, and its updater signature to
   `artifacts\`. It signs with `%USERPROFILE%\.prism\signing\updater.key`.

   ```powershell
   bun install                       # Tauri CLI, used only to sign the installer
   .\packaging\build-installer.ps1
   ```

3. Add `Prism_<version>_release-notes.md` to `artifacts\`.
4. Tag and publish:

   ```powershell
   .\packaging\check-version.ps1 -ExpectedTag <version>
   git tag -a <version> -m "Prism <version>"
   git push origin main <version>
   .\packaging\publish-release.ps1 -Tag <version>
   ```

The publisher creates or repairs the GitHub Release. It uploads the installer, its `.sig` file, and
`latest.json`, and uses the release notes as the release body. Then it verifies the public updater
manifest and the installer.

> [!WARNING]
> Keep `updater.key` and its password in a safe backup. Installed copies of Prism reject updates that
> a different key signs. Only the public key goes in the repository.

## License

[MIT](LICENSE)
