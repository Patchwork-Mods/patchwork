use super::*;

// `solid.rar` and `crypted.rar` are from the `unrar` crate's tests; the rest
// were made with 7-Zip.
const SEVEN_ZIP: &[u8] = include_bytes!("fixtures/mod.7z");
const SOLID_RAR: &[u8] = include_bytes!("fixtures/solid.rar");
const PROTECTED: [(&str, &[u8]); 4] = [
    ("crypted.rar", include_bytes!("fixtures/crypted.rar")),
    ("encrypted.7z", include_bytes!("fixtures/encrypted.7z")),
    (
        "encrypted-names.7z",
        include_bytes!("fixtures/encrypted-names.7z"),
    ),
    ("encrypted.zip", include_bytes!("fixtures/encrypted.zip")),
];

/// Unpacks an archive with `contents`, returning where to and the error if
/// it failed.
fn unpack_bytes(contents: &[u8]) -> (tempfile::TempDir, PathBuf, Result<()>) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("archive");
    fs::write(&archive, contents).unwrap();
    let dest = tmp.path().join("out");
    let result = unpack(&archive, &dest);
    (tmp, dest, result)
}

/// Every file under `dir`, with its contents, and whether any symlinks were
/// found.
fn unpacked(dir: &Path) -> (Vec<(String, Vec<u8>)>, bool) {
    let mut files = Vec::new();
    let mut links = false;
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry.unwrap();
        links |= entry.path_is_symlink();
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(dir).unwrap();
            let rel = rel.to_string_lossy().replace('\\', "/");
            files.push((rel, fs::read(entry.path()).unwrap()));
        }
    }
    (files, links)
}

#[test]
fn seven_zips_are_unpacked() {
    let (_tmp, dest, result) = unpack_bytes(SEVEN_ZIP);
    result.unwrap();
    let (files, _) = unpacked(&dest);
    let names: Vec<_> = files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Data/empty.txt",
            "Data/mod.esp",
            "Data/textures/armor/big.dds",
            "Data/ünïcode ✓.txt",
        ]
    );
    assert_eq!(files[1].1, b"esp");
    // big enough to span more than one read, and solid
    let big = String::from_utf8(files[2].1.clone()).unwrap();
    assert_eq!(big.lines().count(), 20000);
    assert_eq!(big.lines().last(), Some("texel 20000"));
    assert_eq!(files[3].1, "héllo".as_bytes());
}

#[test]
fn rars_are_unpacked() {
    let (_tmp, dest, result) = unpack_bytes(SOLID_RAR);
    result.unwrap();
    let (files, _) = unpacked(&dest);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].0, ".gitignore");
    assert_eq!(files[0].1.len(), 18);

    let (_tmp, dest, result) = unpack_bytes(&rar5(&[
        Rar5Entry::Dir("Data"),
        Rar5Entry::File("Data/meshes/a.nif", b"mesh"),
        Rar5Entry::File("readme.txt", b""),
    ]));
    result.unwrap();
    let (files, _) = unpacked(&dest);
    assert_eq!(
        files,
        [
            ("Data/meshes/a.nif".to_owned(), b"mesh".to_vec()),
            ("readme.txt".to_owned(), Vec::new()),
        ]
    );
}

#[test]
fn password_protected_archives_say_so() {
    for (name, contents) in PROTECTED {
        let (_tmp, _, result) = unpack_bytes(contents);
        let err = result.unwrap_err().to_string();
        assert!(err.contains("password-protected"), "{name}: {err}");
    }
}

#[test]
fn only_known_kinds_of_archive_are_unpacked() {
    for contents in [&b"just some text"[..], b"", b"Rar!"] {
        let (_tmp, _, result) = unpack_bytes(contents);
        let err = result.unwrap_err().to_string();
        assert!(err.contains("isn't a zip, 7z or RAR"), "{err}");
    }
}

#[test]
fn rar_entries_cant_escape() {
    let (tmp, _, result) = unpack_bytes(&rar5(&[Rar5Entry::File("../escape.txt", b"x")]));
    assert!(result.is_err());
    assert!(!tmp.path().join("escape.txt").exists());

    // symlinks, one out of the archive with a file that would be written
    // through it, and one unrar thinks is safe
    let (tmp, dest, result) = unpack_bytes(&rar5(&[
        Rar5Entry::Link("link", "../outside"),
        Rar5Entry::File("link/inside.txt", b"x"),
        Rar5Entry::Link("safe", "link"),
    ]));
    fs::create_dir_all(tmp.path().join("outside")).unwrap();
    result.unwrap();
    let (files, links) = unpacked(&dest);
    assert!(!links);
    assert_eq!(files, [("link/inside.txt".to_owned(), b"x".to_vec())]);
    assert_eq!(fs::read_dir(tmp.path().join("outside")).unwrap().count(), 0);
}

enum Rar5Entry<'a> {
    Dir(&'a str),
    File(&'a str, &'a [u8]),
    /// A Unix symlink to the path given.
    Link(&'a str, &'a str),
}

/// A RAR 5 archive of `entries`, stored without compression. There's no
/// way to write RAR archives from Rust, and this makes it possible to test
/// what a real one couldn't hold, like entries outside the archive.
fn rar5(entries: &[Rar5Entry<'_>]) -> Vec<u8> {
    let mut out = b"Rar!\x1a\x07\x01\x00".to_vec();
    // the main archive header, with no flags
    rar5_block(&mut out, 1, &vint(0), &[], None);
    for entry in entries {
        let (name, data, mode, dir, link) = match *entry {
            Rar5Entry::Dir(name) => (name, None, 0o40755, true, None),
            Rar5Entry::File(name, data) => (name, Some(data), 0o100644, false, None),
            Rar5Entry::Link(name, to) => (name, Some(&b""[..]), 0o120777, false, Some(to)),
        };
        let data_len = data.map_or(0, <[u8]>::len) as u64;
        // directory, or else has a CRC
        let mut fields = vint(if dir { 0x1 } else { 0x4 });
        fields.extend(vint(data_len));
        fields.extend(vint(mode));
        if let Some(data) = data {
            fields.extend(crc32(data).to_le_bytes());
        }
        fields.extend(vint(0)); // stored
        fields.extend(vint(1)); // made on Unix
        fields.extend(vint(name.len() as u64));
        fields.extend(name.as_bytes());
        let mut extra = Vec::new();
        if let Some(to) = link {
            // a file system redirection record: a Unix symlink
            let mut record = vint(5);
            record.extend(vint(1));
            record.extend(vint(0));
            record.extend(vint(to.len() as u64));
            record.extend(to.as_bytes());
            extra.extend(vint(record.len() as u64));
            extra.extend(record);
        }
        rar5_block(&mut out, 2, &fields, &extra, data);
    }
    // end of archive
    rar5_block(&mut out, 5, &vint(0), &[], None);
    out
}

fn rar5_block(out: &mut Vec<u8>, kind: u64, fields: &[u8], extra: &[u8], data: Option<&[u8]>) {
    let mut flags = 0;
    let mut sizes = Vec::new();
    if !extra.is_empty() {
        flags |= 0x1;
        sizes.extend(vint(extra.len() as u64));
    }
    if let Some(data) = data {
        flags |= 0x2;
        sizes.extend(vint(data.len() as u64));
    }
    let mut header = vint(kind);
    header.extend(vint(flags));
    header.extend(sizes);
    header.extend(fields);
    header.extend(extra);
    let mut sized = vint(header.len() as u64);
    sized.extend(header);
    out.extend(crc32(&sized).to_le_bytes());
    out.extend(sized);
    out.extend(data.unwrap_or_default());
}

fn vint(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}
