//! Embeds every file under `assets/` (icons, taskbar glyphs) into the crate as
//! a sorted `(path, bytes)` table that the GPUI asset source serves.

use std::path::{Path, PathBuf};

fn collect(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, root, out);
        } else {
            out.push(path.strip_prefix(root).expect("under root").to_path_buf());
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir")).join("assets");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();
    let mut code = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for file in files {
        let key = file.to_string_lossy().replace('\\', "/");
        let full = root.join(&file);
        code.push_str(&format!("    ({key:?}, include_bytes!({:?})),\n", full.to_string_lossy()));
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("assets.rs");
    std::fs::write(out, code).expect("write asset table");
}
