# Prism

[![Latest release](https://img.shields.io/github/v/release/Hi9841/Prism)](https://github.com/Hi9841/Prism/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform: Windows 10 | 11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4)

Prism is a fast, keyboard-first command palette and taskbar companion for Windows. Use it to find
applications, files, folders, and Windows tools, and to change the Windows 11 taskbar. Prism is a native
Rust app with a [GPUI](https://www.gpui.rs/) interface. It does not use a web view.

[Demo attachment](https://github.com/user-attachments/assets/ec4ac19a-8868-4a01-b9f4-10008c297b6c)

## Features

- Search installed desktop apps and packaged (Microsoft Store) apps.
- Search Windows tools, Control Panel applets, and installed power plans.
- Find local files and folders with fuzzy matching and direct path browsing.
- Pin and reorder apps, and keep up to six Quick Access folders.
- Open Prism with a global shortcut or the Windows Start button.
- Run eligible apps and scripts as administrator from the context menu.
- Change taskbar alignment, icon density, button grouping, auto-hide, and the Start button icon.
- Follow the Windows theme, or select a light or dark appearance and an accent color.
- Install signed updates from the launcher footer.

## Install

Download the latest `Prism_<version>_x64-setup.exe` from
[GitHub Releases](https://github.com/Hi9841/Prism/releases/latest).

- Windows 10 or Windows 11, x64.
- Installs for the current user. Administrator access is not necessary.
- WebView2 is not necessary.

The installer does not have an Authenticode signature, so SmartScreen can show a warning. Prism verifies
in-app updates with its own updater key (minisign).

## Usage

1. Open Prism with the Windows key or your shortcut.
2. Type an app name, a file name, a folder path, a Windows tool, or a calculation. For example:
   `device manager`, `performance options`, or `power plan`.
3. Use the arrow keys to select a result, then push Enter.

Prism starts hidden when you sign in to Windows, so the shortcut is ready immediately. To change this,
open **Task Manager > Startup apps**.

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

Prism changes the taskbar directly through Windows. StartAllBack is not necessary.

- Align the taskbar apps to the left or to the center.
- Select Compact, Default, or When full icon density.
- Set auto-hide and taskbar button grouping.
- Select the System, Gem, or Diamond Start button icon, or add your own PNG (`96 x 96`, transparent
  background).

Windows 11 controls the height of the taskbar. The density setting changes the icon scale only.

### Updates

Prism checks GitHub Releases when it starts, when the menu opens, and one time each hour. When a newer
signed build is available, an update control shows in the launcher footer.

## Build from source

Requirements:

- Windows 10 or Windows 11, x64
- Rust stable with the MSVC toolchain, Clippy, and rustfmt
- Visual Studio Build Tools with the **Desktop development with C++** workload
- [Bun](https://bun.sh/) 1.3.14 (behavior judge and installer signing)

Build and run:

```powershell
cargo run -p prism-app            # builds target\debug\prism.exe and starts it
```

Check:

```powershell
cargo check --workspace --all-targets
cargo build -p prism-judge
bun migration/judge/judge.mjs     # 277 behavior scenarios
```

`PRISM_ADAPTER_CMD=<exe>` runs the judge against a different adapter.

## Project structure

```
crates/
  prism-app/      prism.exe: window, global hotkey, tray, updater host
  prism-ui/       views and state
  prism-core/     backend: Windows integration, file index, search, taskbar, audio
  prism-judge/    JSON adapter that the behavior judge drives
vendor/gpui/      GPUI 0.2.2 with Prism's patches (PRISM-PATCHES.md)
packaging/        installer build, NSIS script, version check, release publisher
migration/        behavior judge, design rulebook, records of the port from React/Tauri
oploop/           performance ledger, benchmarks, functional test of the real app
```

## Release

1. Set the same version in `Cargo.toml` (`[workspace.package]`) and `crates/prism-core/Cargo.toml`, then
   run `cargo update --workspace` to update `Cargo.lock`.
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

## History

Prism 1.0.0 to 1.0.11 replaced the original React/Tauri app with this native port. The original app
left the repository after 1.0.11. Git history keeps it, and `migration/` keeps the records of the port.

## License

[MIT](LICENSE)
