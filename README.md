# Prism

Prism is a fast, keyboard-first command palette and Windows taskbar companion. Use Prism to find local applications, files, and folders. Prism keeps your frequent locations near. You can also change the Windows 11 taskbar.

[Demo attachment](https://github.com/user-attachments/assets/ec4ac19a-8868-4a01-b9f4-10008c297b6c)

## Highlights

- Search installed desktop and packaged Windows applications.
- Search Windows tools, Control Panel applets, and installed power plans.
- Find local files and folders with fuzzy matching and direct path browsing.
- Pin applications, reorder them, and keep up to six Quick Access folders.
- Open Prism from the configured global shortcut or the Windows Start button.
- Right-click eligible applications and scripts to run them as administrator.
- Customize taskbar alignment, icon density, button grouping, auto-hide, and the Start button icon.
- Follow the Windows theme or choose a light or dark appearance and an accent color.
- Install signed updates from the Prism footer without downloading a new installer manually.

## Download

Download the latest `Prism_*_x64-setup.exe` from [GitHub Releases](https://github.com/Hi9841/Prism/releases/latest).

Prism supports Windows 10 and Windows 11 on x64. It installs for the current user and does not require administrator access. It does not need WebView2.

The installer does not have a Windows Authenticode signature, so SmartScreen can show a warning. Prism verifies in-app updates separately with Prism's committed updater public key (minisign).

## Using Prism

1. Open Prism with the Windows key or your configured shortcut.
2. Type an application name, file name, folder path, Windows tool, or calculation. Try `device manager`, `performance options`, or `power plan`.
3. Use the arrow keys to select a result and press Enter to open it.

The launcher starts hidden when you sign in to Windows so the global shortcut is ready immediately. To change the startup behavior, open **Task Manager > Startup apps**.

### Run as administrator

Right-click an eligible application or script and select **Run as administrator**. The same menu is available with `Shift+F10` or the keyboard Menu key. Windows displays its standard UAC consent or credential prompt before launching the target.

Supported local targets:

- Applications: `.exe`, `.com`
- Batch scripts: `.bat`, `.cmd`
- PowerShell scripts: `.ps1`
- Windows Script Host files: `.vbs`, `.js`, `.wsf`

Packaged Microsoft Store apps use Windows application activation and do not expose the administrator action.

## Taskbar customization

Prism applies taskbar changes directly through Windows. StartAllBack is not required.

- Align the taskbar application group to the left or center.
- Choose Compact, Default, or When full icon density.
- Configure auto-hide and taskbar button grouping.
- Select the System, Gem, or Diamond Start button icon.
- Add, reuse, and remove your own Start button PNGs.

For custom Start icons, use a `96 x 96` PNG with a transparent background. Windows 11 controls the height of the taskbar surface. The Prism density setting changes the icon scale. It does not force an unsupported taskbar height.

## Updates

Prism checks the latest GitHub Release when it starts, whenever the menu opens, and once per hour as a fallback. When a newer signed build is available, an update control appears in the launcher footer.

Each updater-enabled release contains:

- `Prism_<version>_x64-setup.exe`
- `Prism_<version>_x64-setup.exe.sig`
- `latest.json`

People installing Prism manually only need the `.exe`. The signature and manifest are consumed automatically by the updater.

## Development

Prism is a native Rust app with a [GPUI](https://www.gpui.rs/) interface. All code is in `gpui/`, one Cargo workspace.

Requirements:

- Windows 10 or Windows 11 on x64
- Rust stable with the MSVC toolchain, Clippy, and rustfmt
- Visual Studio Build Tools with the **Desktop development with C++** workload
- [Bun](https://bun.sh/) 1.3.14 for the behavior judge and the installer signing tool

Build and run:

```powershell
cd gpui
cargo build -p prism-app        # target\debug\prism.exe
```

Run the checks:

```powershell
cd gpui
cargo check --workspace --all-targets
cargo build -p prism-judge
bun migration/judge/judge.mjs   # 277 behavior scenarios against the port
```

## Build the installer

```powershell
cd gpui
bun install                     # the Tauri CLI, used only to sign the installer
.\packaging\build-installer.ps1
```

The script builds the release exe, the NSIS installer (`packaging\nsis`), and its updater signature with
`%USERPROFILE%\.prism\signing\updater.key`. It writes them to `gpui\artifacts\`.

## Publish a release

Keep the version identical in `gpui\Cargo.toml` (`[workspace.package]`), `gpui\crates\prism-core\Cargo.toml`,
and `gpui\Cargo.lock` (`cargo update --workspace`). Add `Prism_<version>_release-notes.md` to `gpui\artifacts`,
build the installer, then publish from a matching tag:

```powershell
cd gpui
.\packaging\check-version.ps1 -ExpectedTag <version>
git tag -a <version> -m "Prism <version>"
git push origin main <version>
.\packaging\publish-release.ps1 -Tag <version>
```

The publisher creates or repairs the GitHub Release, uploads the installer, its signature, and `latest.json`, and
uses the Markdown file as the release body. Then it verifies the public updater manifest and the installer.

Keep `%USERPROFILE%\.prism\signing\updater.key` and its password backed up securely. Existing Prism installations
reject updates signed by a replacement key. Only the public key belongs in the repository.

## Repository layout

- `gpui/crates/prism-app/` - the `prism.exe` shell: window, hotkey, tray, updater host
- `gpui/crates/prism-ui/` - views and state
- `gpui/crates/prism-core/` - the backend: native Windows integration, indexing, and launcher behavior
- `gpui/crates/prism-judge/` - the adapter that the behavior judge drives
- `gpui/vendor/gpui/` - GPUI 0.2.2 with Prism's patches (`PRISM-PATCHES.md`)
- `gpui/packaging/` - the installer script, the version check, and the release publisher
- `gpui/migration/` - the behavior judge and the migration records (design rulebook, handoff)
- `gpui/oploop/` - the performance ledger, benchmarks, and the functional test of the real app

Prism 1.0.0 to 1.0.11 replaced the original React/Tauri app, which was the behavior spec of the port. That app
left the repository after release 1.0.11; its history stays in git.

## License

Prism is available under the [MIT License](LICENSE).
