use std::path::PathBuf;
use std::process::Command;

fn main() {
    build_shell_hook();
    embed_app_manifest();
    // tauri's default app manifest is replaced by `embed_app_manifest` (same
    // content, but linker-applied so test binaries get it too). Icon and
    // version resources still come from tauri_build.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
    )
    .expect("failed to run tauri-build");
}

/// The lib links `TaskDialogIndirect`/`SetWindowSubclass` from comctl32, which
/// only exist in Common-Controls v6. tauri_build embeds the v6 dependency
/// manifest through `rustc-link-arg-bins`, which never reaches test binaries -
/// so `cargo test` died at load time with STATUS_ENTRYPOINT_NOT_FOUND before
/// running a single test. This replaces that resource-based manifest with the
/// same content (identical to tauri's windows-app-manifest.xml) applied to
/// every linked target, tests included. tauri's remaining resources (app
/// icon, version info) are unaffected.
fn embed_app_manifest() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") || !target.contains("msvc") {
        return;
    }
    let out_dir =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo did not provide OUT_DIR"));
    let manifest = out_dir.join("prism-app-manifest.xml");
    std::fs::write(
        &manifest,
        r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#,
    )
    .expect("failed to write the Prism app manifest");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}

fn build_shell_hook() {
    println!("cargo:rerun-if-changed=shell-hook/src/lib.rs");

    let target = std::env::var("TARGET").expect("Cargo did not provide TARGET");
    if !target.contains("windows") {
        return;
    }

    let source = PathBuf::from("shell-hook/src/lib.rs");
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo did not provide OUT_DIR"))
        .join("prism_shell_hook.dll");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = Command::new(rustc)
        .arg("--crate-name=prism_shell_hook")
        .arg("--crate-type=cdylib")
        .arg("--edition=2021")
        .arg("--target")
        .arg(target)
        .arg("-Cpanic=abort")
        .arg("-Copt-level=s")
        .arg("-o")
        .arg(&output)
        .arg(source)
        .status()
        .expect("failed to start rustc for the Prism shell hook");
    assert!(status.success(), "failed to build the Prism shell hook");
    println!("cargo:rustc-env=PRISM_SHELL_HOOK_DLL={}", output.display());
}
