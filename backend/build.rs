//! Desktop builds (`--features desktop`) carry the built UI inside the program: an installed app
//! has no frontend/dist next to it. The server's build reads frontend/dist from disk and skips this.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_none() {
        return;
    }
    let dist = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../frontend/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    if !dist.join("index.html").exists() {
        panic!("the desktop build carries the UI: build it first (cd frontend && npm ci && npm run build)");
    }
    let mut files = vec![];
    walk(&dist, &dist, &mut files);
    files.sort();
    let mut out = String::from(
        "/// The built UI (frontend/dist): its paths and contents.\npub static FILES: &[(&str, &[u8])] = &[\n",
    );
    for (name, path) in files {
        out.push_str(&format!("    ({name:?}, include_bytes!({path:?})),\n"));
    }
    out.push_str("];\n");
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ui.rs"), out).unwrap();
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(root, &path, files);
        } else {
            let name = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            files.push((name, path.canonicalize().unwrap().to_string_lossy().into_owned()));
        }
    }
}
