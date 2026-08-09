use std::path::PathBuf;
use std::process::Command;

fn main() {
    build_shell_hook();
    tauri_build::build();
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
