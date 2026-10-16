//! Generates the list of built-in registry entries from `registry/`, so adding
//! a definition there doesn't also need a code change.

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../registry");
    // Watching the directories (not just the files) picks up added and removed
    // files as well as edits.
    println!("cargo:rerun-if-changed={}", root.display());

    let mut entries: Vec<(String, String, PathBuf)> = Vec::new();
    for kind_dir in read_dir(&root).filter(|p| p.is_dir()) {
        println!("cargo:rerun-if-changed={}", kind_dir.display());
        let kind = file_name(&kind_dir);
        for path in read_dir(&kind_dir) {
            if path.extension().is_some_and(|ext| ext == "toml") {
                let id = path.file_stem().unwrap().to_string_lossy().into_owned();
                entries.push((kind.clone(), id, path.canonicalize().unwrap()));
            }
        }
    }
    entries.sort();

    let mut out =
        String::from("/// `(kind, id, contents)`\npub const ENTRIES: &[(&str, &str, &str)] = &[\n");
    for (kind, id, path) in &entries {
        let path = path.to_str().expect("registry paths must be UTF-8");
        writeln!(out, "    ({kind:?}, {id:?}, include_str!({path:?})),").unwrap();
    }
    out.push_str("];\n");

    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("builtin.rs");
    fs::write(dest, out).unwrap();
}

fn read_dir(dir: &Path) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}
