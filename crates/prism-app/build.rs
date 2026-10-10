//! Embeds the Prism icon. The application manifest (Common Controls v6 for
//! prism-core's TaskDialogIndirect and SetWindowSubclass, PerMonitorV2 DPI)
//! comes from gpui's default `windows-manifest` feature.

fn main() {
    println!("cargo:rerun-if-changed=resources/prism.rc");
    println!("cargo:rerun-if-changed=resources/icon.ico");
    let _ = embed_resource::compile("resources/prism.rc", embed_resource::NONE);
}
