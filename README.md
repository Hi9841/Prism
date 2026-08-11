# Prism

Prism is a fast, keyboard-first command palette and Windows taskbar companion. Search local applications, files, and folders; keep frequent locations close; and customize the Windows 11 taskbar without StartAllBack.

[Demo attachment](https://github.com/user-attachments/assets/ec4ac19a-8868-4a01-b9f4-10008c297b6c)

## Highlights

- Search installed desktop and packaged Windows applications.
- Find local files and folders with fuzzy matching and direct path browsing.
- Pin applications, reorder them, and keep up to six Quick Access folders.
- Open Prism from the configured global shortcut or the Windows Start button.
- Right-click eligible applications and scripts to run them as administrator.
- Customize taskbar alignment, icon density, button grouping, auto-hide, and the Start button icon.
- Follow the Windows theme or choose a light, dark, acrylic, mica, or solid appearance.
- Install signed updates from the Prism footer without downloading a new installer manually.

## Download

Download the latest `Prism_*_x64-setup.exe` from [GitHub Releases](https://github.com/Hi9841/Prism/releases/latest).

Prism supports Windows 10 and Windows 11 on x64. It installs for the current user and does not require administrator access. If WebView2 is missing, the installer downloads the Microsoft bootstrapper automatically.

The installer is not currently Windows Authenticode-signed, so SmartScreen may display a warning. In-app updates are verified separately with Prism's committed Tauri updater public key.

## Using Prism

1. Open Prism with the Windows key or your configured shortcut.
2. Type an application name, file name, folder path, or calculation.
3. Use the arrow keys to select a result and press Enter to open it.

The launcher starts hidden when you sign in to Windows so the global shortcut is ready immediately. Startup behavior can be managed from **Task Manager > Startup apps**.

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

For custom Start icons, use a `96 x 96` PNG with a transparent background. Windows 11 controls the taskbar surface height; Prism's density setting changes the icon scale rather than forcing an unsupported taskbar height.

## Updates

Prism checks the latest GitHub Release when it starts, whenever the menu opens, and once per hour as a fallback. When a newer signed build is available, an update control appears in the launcher footer.

Each updater-enabled release contains:

- `Prism_<version>_x64-setup.exe`
- `Prism_<version>_x64-setup.exe.sig`
- `latest.json`

People installing Prism manually only need the `.exe`. The signature and manifest are consumed automatically by the updater.

## Development

Requirements:

- Windows 10 or Windows 11 on x64
- [Bun](https://bun.sh/) 1.3.14
- Rust stable with the MSVC toolchain, Clippy, and rustfmt
- Visual Studio Build Tools with the **Desktop development with C++** workload

Install dependencies and start the Tauri app:

```powershell
bun install --frozen-lockfile
bun run tauri dev
```

Run the complete local validation suite:

```powershell
bun run lint
bun run test
bun run build
.\scripts\check-version.ps1
cargo fmt --manifest-path src-tauri\Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri\Cargo.toml
```

## Build the installer

Generate the installer artwork, load the updater signing key, and build the NSIS bundle:

```powershell
.\scripts\generate-installer-assets.ps1
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.prism\signing\updater.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = Get-Content -Raw "$env:USERPROFILE\.prism\signing\updater.key.password"
bun run tauri build
```

The installer and updater signature are written to `src-tauri\target\release\bundle\nsis\`.

## Publish a release

Keep the version identical in `package.json`, `src-tauri\Cargo.toml`, `src-tauri\tauri.conf.json`, and the Prism package entry in `src-tauri\Cargo.lock`. Add the signed installer, signature, and `Prism_<version>_release-notes.md` to `artifacts`, then publish from a matching tag:

```powershell
$version = (Get-Content -Raw src-tauri\tauri.conf.json | ConvertFrom-Json).version
.\scripts\check-version.ps1 -ExpectedTag $version
git tag $version
git push origin $version
.\scripts\publish-release.ps1 -Tag $version
```

The publisher creates or repairs the GitHub Release, uploads all updater assets, uses the Markdown file as the release body, and verifies the public updater endpoint and installer size.

Keep `%USERPROFILE%\.prism\signing\updater.key` and its password backed up securely. Existing Prism installations reject updates signed by a replacement key. Only the public key belongs in the repository.

## Repository layout

- `src/` - React interface, state, and frontend tests
- `src-tauri/src/` - native Windows integration, indexing, and launcher behavior
- `src-tauri/icons/` - application and Windows bundle icons
- `src-tauri/nsis/` - installer artwork and lifecycle hooks
- `scripts/` - version checks, asset generation, release publishing, and cleanup
- `artifacts/` - local installers, signatures, manifests, and release notes

Generated directories such as `dist/`, `src-tauri/gen/`, and `src-tauri/target/` are disposable. Remove them with `bun run clean`.

## License

Prism is available under the [MIT License](LICENSE).
