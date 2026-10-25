use std::fs::File;
use std::io::Write;

use super::*;

fn write_zip(path: &Path, entries: &[(&str, &str)]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    for (name, contents) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn zips_are_unpacked_with_normalized_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("mod.zip");
    write_zip(&archive, &[("plugins\\a.dll", "a"), ("readme.md", "hi")]);

    let store = Store::new(tmp.path().join("store"));
    let hash = store
        .ingest_path(&archive, ArtifactFormat::Archive)
        .unwrap();
    assert!(store.contains(&hash, ArtifactFormat::Archive));
    let files = store
        .files(&hash, ArtifactFormat::Archive, "mod.zip")
        .unwrap();
    let names: Vec<_> = files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(names, ["plugins/a.dll", "readme.md"]);
    assert_eq!(fs::read_to_string(&files[0].src).unwrap(), "a");
}

#[test]
fn zip_slip_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("evil.zip");
    write_zip(&archive, &[("../../escape.txt", "x")]);

    let store = Store::new(tmp.path().join("store"));
    let hash = store
        .ingest_path(&archive, ArtifactFormat::Archive)
        .unwrap();
    assert!(
        store
            .files(&hash, ArtifactFormat::Archive, "evil.zip")
            .is_err()
    );
    assert!(!tmp.path().join("escape.txt").exists());
}

#[test]
fn archives_are_told_apart_by_their_contents() {
    let tmp = tempfile::tempdir().unwrap();
    // a 7z called .zip, as sites sometimes have them
    let archive = tmp.path().join("mod.zip");
    fs::write(&archive, include_bytes!("unpack/fixtures/mod.7z")).unwrap();
    let store = Store::new(tmp.path().join("store"));
    let hash = store
        .ingest_path(&archive, ArtifactFormat::Archive)
        .unwrap();
    let files = store
        .files(&hash, ArtifactFormat::Archive, "mod.zip")
        .unwrap();
    assert_eq!(files.len(), 4);
    assert!(files.iter().any(|f| f.rel.as_str() == "Data/mod.esp"));
}

#[test]
fn identical_dirs_share_a_hash() {
    let tmp = tempfile::tempdir().unwrap();
    for name in ["a", "b"] {
        let dir = tmp.path().join(name).join("sub");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("f.txt"), "same").unwrap();
    }
    let store = Store::new(tmp.path().join("store"));
    let a = store
        .ingest_path(&tmp.path().join("a"), ArtifactFormat::Dir)
        .unwrap();
    let b = store
        .ingest_path(&tmp.path().join("b"), ArtifactFormat::Dir)
        .unwrap();
    assert_eq!(a, b);
    fs::write(tmp.path().join("b/sub/f.txt"), "different").unwrap();
    let b = store
        .ingest_path(&tmp.path().join("b"), ArtifactFormat::Dir)
        .unwrap();
    assert_ne!(a, b);
}
