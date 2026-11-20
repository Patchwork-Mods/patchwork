use std::fmt::Write;
use std::path::Path;

fn main() {
    registry_themes();
    runtime_paths();
    tauri_build::build();
}

/// Builds in where to find GTK/WebKit and GIO's modules, when the build
/// says (the Nix dev shell does), so the binary runs without the shell's
/// `LD_LIBRARY_PATH` and `GIO_MODULE_DIR`, as it must when the browser
/// starts it for an `nxm://` link.
fn runtime_paths() {
    println!("cargo:rerun-if-env-changed=PATCHWORK_GUI_RPATH");
    println!("cargo:rerun-if-env-changed=PATCHWORK_GUI_GIO_MODULE_DIR");
    if let Ok(rpath) = std::env::var("PATCHWORK_GUI_RPATH")
        && !rpath.is_empty()
    {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{rpath}");
    }
}

/// Lists the themes in `registry/themes/` (games' own themes, which come
/// with the built-in registry) for `themes.rs` to build in, so adding one
/// there doesn't also need a code change. A theme is `<id>.css`, or a
/// folder `<id>/` with its CSS in `theme.css` and its files beside it.
fn registry_themes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../registry/themes");
    // Watching the directory picks up added, removed and changed files in
    // it and its folders.
    println!("cargo:rerun-if-changed={}", dir.display());
    let utf8 = |p: &Path| {
        p.canonicalize()
            .unwrap()
            .to_str()
            .expect("registry paths must be UTF-8")
            .to_owned()
    };
    let mut themes = Vec::new();
    let mut files = Vec::new();
    for path in std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
    {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            let css = path.join("theme.css");
            assert!(css.is_file(), "{} has no theme.css", path.display());
            themes.push((name.clone(), utf8(&css), true));
            folder_files(&path, &path, &mut |rel, file| {
                if rel != "theme.css" {
                    files.push((name.clone(), rel, utf8(file)));
                }
            });
        } else if let Some(id) = name.strip_suffix(".css") {
            themes.push((id.to_owned(), utf8(&path), false));
        }
    }
    themes.sort();
    files.sort();

    let mut out = String::from(
        "/// `(id, css, whether it's a folder)`\nconst REGISTRY: &[(&str, &str, bool)] = &[\n",
    );
    for (id, path, folder) in &themes {
        writeln!(out, "    ({id:?}, include_str!({path:?}), {folder}),").unwrap();
    }
    out.push_str("];\n\n/// `(theme id, path in its folder, contents)`\n");
    out.push_str("pub(crate) const REGISTRY_FILES: &[(&str, &str, &[u8])] = &[\n");
    for (id, rel, path) in &files {
        writeln!(out, "    ({id:?}, {rel:?}, include_bytes!({path:?})),").unwrap();
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("registry_themes.rs");
    std::fs::write(dest, out).unwrap();
}

/// Calls `f` with each file under `dir`, and its path from `root` with `/`
/// between the parts. Hidden files are left out.
fn folder_files(root: &Path, dir: &Path, f: &mut dyn FnMut(String, &Path)) {
    for path in std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()) {
        if path.file_name().unwrap().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            folder_files(root, &path, f);
        } else {
            let rel: Vec<String> = path
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            f(rel.join("/"), &path);
        }
    }
}
