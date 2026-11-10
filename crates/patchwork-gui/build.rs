use std::fmt::Write;
use std::path::Path;

fn main() {
    registry_themes();
    tauri_build::build();
}

/// Lists the themes in `registry/themes/` (games' own themes, which come
/// with the built-in registry) for `themes.rs` to build in, so adding one
/// there doesn't also need a code change.
fn registry_themes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../registry/themes");
    // Watching the directory picks up added and removed files too.
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut themes: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "css"))
        .map(|p| {
            let id = p.file_stem().unwrap().to_string_lossy().into_owned();
            let path = p.canonicalize().unwrap();
            let path = path
                .to_str()
                .expect("registry paths must be UTF-8")
                .to_owned();
            (id, path)
        })
        .collect();
    themes.sort();

    let mut out = String::from("/// `(id, css)`\nconst REGISTRY: &[(&str, &str)] = &[\n");
    for (id, path) in &themes {
        writeln!(out, "    ({id:?}, include_str!({path:?})),").unwrap();
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("registry_themes.rs");
    std::fs::write(dest, out).unwrap();
}
