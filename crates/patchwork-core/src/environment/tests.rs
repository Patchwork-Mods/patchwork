use super::*;

/// A minimal PE32+ executable with nothing in it but a version resource for
/// `version`.
fn fake_exe(version: [u16; 4]) -> Vec<u8> {
    const RSRC_RVA: u32 = 0x1000;
    const RSRC_AT: usize = 0x200;
    let put16 =
        |buf: &mut Vec<u8>, at: usize, v: u16| buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
    let put32 =
        |buf: &mut Vec<u8>, at: usize, v: u32| buf[at..at + 4].copy_from_slice(&v.to_le_bytes());

    // Type, name and language directories, each with one entry, then the
    // data entry and the VS_VERSIONINFO it points to.
    let mut rsrc = vec![0; 88];
    for (dir, id, next) in [
        (0, 16, 24 | 0x8000_0000),
        (24, 1, 48 | 0x8000_0000),
        (48, 0x409, 72),
    ] {
        put16(&mut rsrc, dir + 14, 1);
        put32(&mut rsrc, dir + 16, id);
        put32(&mut rsrc, dir + 20, next);
    }
    let mut info = vec![0; 40 + 52];
    put16(&mut info, 2, 52);
    for (i, c) in "VS_VERSION_INFO".encode_utf16().enumerate() {
        put16(&mut info, 6 + i * 2, c);
    }
    put32(&mut info, 40, 0xfeef04bd);
    put32(
        &mut info,
        48,
        u32::from(version[0]) << 16 | u32::from(version[1]),
    );
    put32(
        &mut info,
        52,
        u32::from(version[2]) << 16 | u32::from(version[3]),
    );
    let len = info.len();
    put16(&mut info, 0, len as u16);
    put32(&mut rsrc, 72, RSRC_RVA + 88);
    put32(&mut rsrc, 76, info.len() as u32);
    rsrc.extend(info);

    let mut exe = vec![0; RSRC_AT];
    exe[..2].copy_from_slice(b"MZ");
    put32(&mut exe, 0x3c, 0x40);
    exe[0x40..0x44].copy_from_slice(b"PE\0\0");
    put16(&mut exe, 0x46, 1); // sections
    put16(&mut exe, 0x54, 240); // optional header size
    let optional = 0x58;
    put16(&mut exe, optional, 0x20b);
    put32(&mut exe, optional + 108, 16);
    put32(&mut exe, optional + 112 + 16, RSRC_RVA);
    put32(&mut exe, optional + 112 + 20, rsrc.len() as u32);
    let section = optional + 240;
    exe[section..section + 5].copy_from_slice(b".rsrc");
    put32(&mut exe, section + 8, rsrc.len() as u32);
    put32(&mut exe, section + 12, RSRC_RVA);
    put32(&mut exe, section + 16, rsrc.len() as u32);
    put32(&mut exe, section + 20, RSRC_AT as u32);
    exe.extend(rsrc);
    exe
}

fn game(detect: &str) -> GameDef {
    toml::from_str(&format!(
        "schema = 1\nid = \"g\"\nname = \"G\"\n[deployer]\nkind = \"rules\"\n[detect]\n{detect}"
    ))
    .unwrap()
}

#[test]
fn reads_the_version_of_an_exe() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Game.exe"), fake_exe([1, 6, 1170, 0])).unwrap();
    assert_eq!(
        exe_version(&tmp.path().join("Game.exe")),
        Some([1, 6, 1170, 0])
    );

    let all = game("[[detect.version]]\nkind = \"exe\"\nfile = \"Game.exe\"");
    assert_eq!(
        GameEnv::detect(&all, tmp.path()).version.as_deref(),
        Some("1.6.1170.0")
    );
    let three = game("[[detect.version]]\nkind = \"exe\"\nfile = \"Game.exe\"\nparts = 3");
    assert_eq!(
        GameEnv::detect(&three, tmp.path()).version.as_deref(),
        Some("1.6.1170")
    );
}

#[test]
fn files_that_arent_executables_have_no_version() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("Game.exe");
    std::fs::write(&path, b"MZ but nothing else").unwrap();
    assert_eq!(exe_version(&path), None);
    let mut truncated = fake_exe([1, 0, 0, 0]);
    truncated.truncate(0x220);
    std::fs::write(&path, truncated).unwrap();
    assert_eq!(exe_version(&path), None);
    assert_eq!(exe_version(&tmp.path().join("missing.exe")), None);
}

#[test]
fn the_first_probe_that_finds_a_version_wins() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("version.json"),
        r#"{"game": {"version": "1.20.1"}}"#,
    )
    .unwrap();
    let def = game(
        "[[detect.version]]\nkind = \"exe\"\nfile = \"missing.exe\"\n\
         [[detect.version]]\nkind = \"json\"\nfile = \"version.json\"\nkey = \"nope\"\n\
         [[detect.version]]\nkind = \"json\"\nfile = \"version.json\"\nkey = \"game.version\"",
    );
    let env = GameEnv::detect(&def, tmp.path());
    assert_eq!(env.version.as_deref(), Some("1.20.1"));
    assert_eq!(env.to_string(), "1.20.1");
}

#[test]
fn probes_cant_read_outside_the_install() {
    let err = toml::from_str::<GameDef>(
        "schema = 1\nid = \"g\"\nname = \"G\"\n[deployer]\nkind = \"rules\"\n\
         [[detect.version]]\nkind = \"json\"\nfile = \"../secret.json\"\nkey = \"a\"",
    )
    .unwrap_err();
    assert!(err.to_string().contains("outside"), "{err}");
}

#[test]
fn steam_installs_know_their_build() {
    let tmp = tempfile::tempdir().unwrap();
    let steamapps = tmp.path().join("steamapps");
    let root = steamapps.join("common/My Game");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        steamapps.join("appmanifest_489830.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"489830\"\n\t\"buildid\"\t\t\"13190634\"\n}\n",
    )
    .unwrap();
    let env = GameEnv::detect(&game("steam = [1, 489830]"), &root);
    assert_eq!(env.steam_build.as_deref(), Some("13190634"));
    assert_eq!(env.to_string(), "Steam build 13190634");
    // not for a game Steam doesn't know as that app
    assert!(GameEnv::detect(&game("steam = [2]"), &root).is_empty());
}
