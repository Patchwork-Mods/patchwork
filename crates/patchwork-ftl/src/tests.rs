use std::io::Cursor;

use super::*;
use crate::xml::{Element, Node};

/// Elements with their attributes and trimmed text, leaving out comments,
/// declarations and whitespace, for comparing documents written by
/// different tools.
fn normalize(nodes: &[Node]) -> Vec<String> {
    fn walk(nodes: &[Node], depth: usize, out: &mut Vec<String>) {
        for node in nodes {
            match node {
                Node::Element(e) => {
                    let mut attrs: Vec<_> =
                        e.attrs.iter().map(|(n, v)| format!("{n}={v:?}")).collect();
                    attrs.sort();
                    out.push(format!(
                        "{}<{} {}>",
                        " ".repeat(depth),
                        e.name,
                        attrs.join(" ")
                    ));
                    walk(&e.children, depth + 1, out);
                }
                Node::Text(t) | Node::CData(t) if !t.trim().is_empty() => {
                    let t = t
                        .trim()
                        .replace("&gt;", ">")
                        .replace("&lt;", "<")
                        .replace("&quot;", "\"")
                        .replace("&apos;", "'")
                        .replace("&amp;", "&");
                    out.push(format!("{}{t:?}", " ".repeat(depth)));
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(nodes, 0, &mut out);
    out
}

/// Builds an archive from `PATCHWORK_FTL_VANILLA` and the mods (zips) in
/// `PATCHWORK_FTL_MODS` (separated by `:`, in load order) and compares it
/// to `PATCHWORK_FTL_EXPECTED`, built by another mod manager from the
/// same: the same files, the same XML once normalized, and the same bytes
/// for everything else.
#[test]
#[ignore = "needs FTL and mods; see the doc comment"]
fn compare_with_another_manager() {
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
    let vanilla = PathBuf::from(var("PATCHWORK_FTL_VANILLA"));
    let expected = PathBuf::from(var("PATCHWORK_FTL_EXPECTED"));
    let tmp = tempfile::tempdir().unwrap();
    let index = pkg::read_index(&mut BufReader::new(File::open(&vanilla).unwrap())).unwrap();
    let mut files = Files::new(&vanilla, index).unwrap();
    let mut warnings = Vec::new();
    for (i, zip) in var("PATCHWORK_FTL_MODS").split(':').enumerate() {
        let dir = tmp.path().join(i.to_string());
        unpack(Path::new(zip), &dir).unwrap();
        let entries = walk(&dir).unwrap();
        files.apply(&entries, &mut warnings).unwrap();
    }
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    let built = tmp.path().join("ftl.dat");
    pkg::write(&built, Some(&vanilla), &files.finish()).unwrap();

    let read = |path: &Path| {
        let mut file = File::open(path).unwrap();
        let index = pkg::read_index(&mut BufReader::new(&mut file)).unwrap();
        let entries: BTreeMap<String, pkg::Entry> = index
            .into_iter()
            .map(|e| (e.path.to_ascii_lowercase(), e))
            .collect();
        (file, entries)
    };
    let (mut ours, our_entries) = read(&built);
    let (mut theirs, their_entries) = read(&expected);
    // Files outside the mod folders (at the top of a mod, say) aren't
    // installed, though some managers do.
    let only = |a: &BTreeMap<String, pkg::Entry>, b: &BTreeMap<String, pkg::Entry>| {
        a.keys()
            .filter(|k| !b.contains_key(*k) && installed(k))
            .cloned()
            .collect::<Vec<_>>()
    };
    let ours_only = only(&our_entries, &their_entries);
    let theirs_only = only(&their_entries, &our_entries);
    let mut differ = Vec::new();
    for (key, ours_e) in &our_entries {
        let Some(theirs_e) = their_entries.get(key) else {
            continue;
        };
        let a = pkg::read_entry(&mut ours, ours_e.offset, ours_e.size).unwrap();
        let b = pkg::read_entry(&mut theirs, theirs_e.offset, theirs_e.size).unwrap();
        if a == b {
            continue;
        }
        if let Ok(dump) = std::env::var("PATCHWORK_FTL_DUMP") {
            let name = key.replace('/', "_");
            fs::write(Path::new(&dump).join(format!("{name}.ours")), &a).unwrap();
            fs::write(Path::new(&dump).join(format!("{name}.theirs")), &b).unwrap();
        }
        if key.ends_with(".xml") {
            let a = normalize(&xml::parse(&xml::decode(&a)));
            let b = normalize(&xml::parse(&xml::decode(&b)));
            if a != b {
                let at = a
                    .iter()
                    .zip(&b)
                    .position(|(x, y)| x != y)
                    .unwrap_or(a.len().min(b.len()));
                differ.push(format!(
                    "{key}: {} vs {} lines, first difference at {at}:\n  ours:   {:?}\n  theirs: {:?}",
                    a.len(),
                    b.len(),
                    a.get(at),
                    b.get(at)
                ));
            }
        } else {
            differ.push(format!("{key}: {} vs {} bytes", a.len(), b.len()));
        }
    }
    eprintln!(
        "{} files ours, {} theirs; only ours: {ours_only:?}; only theirs: {theirs_only:?}",
        our_entries.len(),
        their_entries.len()
    );
    for d in &differ {
        eprintln!("{d}");
    }
    assert!(ours_only.is_empty() && theirs_only.is_empty() && differ.is_empty());
}

#[test]
fn hashes_match_the_games() {
    // From the game's own ftl.dat.
    assert_eq!(pkg::hash("img/map/map_targetbox.png"), 0x0022_5ec8);
    assert_eq!(pkg::hash("img/icons/s_engines_red2.png"), 0x0023_165f);
    assert_eq!(pkg::hash("IMG/Map/MAP_targetbox.png"), 0x0022_5ec8);
}

#[test]
fn archives_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("file");
    fs::write(&file, b"from a file").unwrap();
    let first = tmp.path().join("first.dat");
    pkg::write(
        &first,
        None,
        &[
            ("data/a.xml".to_owned(), Data::Bytes(b"<a/>".to_vec())),
            ("img/b.png".to_owned(), Data::File(file)),
            ("audio/empty.ogg".to_owned(), Data::Bytes(Vec::new())),
        ],
    )
    .unwrap();
    let mut archive = File::open(&first).unwrap();
    let index = pkg::read_index(&mut archive).unwrap();
    let hashes: Vec<u32> = index.iter().map(|e| pkg::hash(&e.path)).collect();
    assert!(hashes.is_sorted());
    assert!(index.iter().all(|e| e.offset % 64 == 0));
    let get = |archive: &mut File, path: &str| {
        let e = index.iter().find(|e| e.path == path).unwrap();
        pkg::read_entry(archive, e.offset, e.size).unwrap()
    };
    assert_eq!(get(&mut archive, "img/b.png"), b"from a file");
    assert_eq!(get(&mut archive, "audio/empty.ogg"), b"");

    // Files can be copied out of one archive into the next.
    let a = index.iter().find(|e| e.path == "data/a.xml").unwrap();
    let second = tmp.path().join("second.dat");
    pkg::write(
        &second,
        Some(&first),
        &[(
            "data/a.xml".to_owned(),
            Data::Base {
                offset: a.offset,
                size: a.size,
            },
        )],
    )
    .unwrap();
    let mut archive = File::open(&second).unwrap();
    let index = pkg::read_index(&mut archive).unwrap();
    assert_eq!(index.len(), 1);
    assert_eq!(
        pkg::read_entry(&mut archive, index[0].offset, index[0].size).unwrap(),
        b"<a/>"
    );

    assert!(pkg::read_index(&mut Cursor::new(b"PK\x03\x04 a zip, not an archive")).is_err());
}

#[test]
fn sloppy_xml_parses_and_writes_back() {
    let text = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
        <!-- a -- comment -->\n\
        <event name=\"A\"><text>Tom &amp; Jerry & co</text>\n\
        <choice req='x\"y'/></event>\n\
        <event name=\"B\"><text>unclosed\n";
    let nodes = xml::parse(text);
    let elements: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
        .collect();
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[0].attr("name"), Some("A"));
    let text_el = elements[0].elements().next().unwrap();
    assert_eq!(text_el.text(), "Tom &amp; Jerry & co");
    let choice = elements[0].elements().nth(1).unwrap();
    assert_eq!(choice.attr("req"), Some("x\"y"));
    // B and its text were never closed, so they end with the file.
    assert_eq!(elements[1].elements().next().unwrap().text(), "unclosed\n");
    let written = xml::write(&nodes);
    assert!(
        written.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- a -- comment -->")
    );
    assert!(written.contains("<choice req='x\"y' />"));
    assert_eq!(xml::parse(&written), nodes);

    // A closing tag that matches nothing open is ignored.
    let nodes = xml::parse("<a><b></c></b></a>");
    assert_eq!(xml::write(&nodes), "<a><b /></a>");
}

#[test]
fn text_is_decoded_from_what_mods_use() {
    assert_eq!(xml::decode(b"\xef\xbb\xbfplain"), "plain");
    assert_eq!(xml::decode(b"\xff\xfeh\0i\0"), "hi");
    assert_eq!(xml::decode(b"caf\xe9 \x93quoted\x94"), "café “quoted”");
}

/// `base` with `patch` appended, normalized.
fn append(base: &str, patch: &str) -> Result<Vec<String>, String> {
    let mut doc = Doc::parse(base);
    let mut warnings = Vec::new();
    doc.append(patch, &mut warnings)?;
    assert!(warnings.is_empty(), "{warnings:?}");
    Ok(normalize(&xml::parse(&doc.write())))
}

fn normalized(text: &str) -> Vec<String> {
    normalize(&xml::parse(text))
}

const EVENTS: &str = r#"<FTL>
<event name="A"><text>first</text></event>
<event name="B" unique="true"><text>b</text><choice><text>ok</text></choice></event>
<event name="A"><text>second</text></event>
<ship name="S" auto_blueprint="X"/>
</FTL>"#;

#[test]
fn plain_content_is_added_inside_the_root() {
    assert_eq!(
        append(EVENTS, r#"<?xml version="1.0"?><event name="C"/>"#).unwrap(),
        normalized(&EVENTS.replace("</FTL>", "<event name=\"C\"/></FTL>"))
    );
    // A patch's own root tags are seen through, even several (and nested
    // or unclosed), as is a file without one.
    assert_eq!(
        append("<a/>", "<FTL><b/></FTL>").unwrap(),
        normalized("<a/><b/>")
    );
    assert_eq!(
        append(
            "<a/>",
            "<FTL><b/><FTL><c/></FTL><!-- x --><FTL><d/></FTL><e/>"
        )
        .unwrap(),
        normalized("<a/><b/><c/><d/><e/>")
    );
}

#[test]
fn find_name_takes_the_last_match_by_default() {
    let patch =
        r#"<mod:findName type="event" name="A"><mod:setAttributes seen="1"/></mod:findName>"#;
    assert_eq!(
        append(EVENTS, patch).unwrap(),
        normalized(&EVENTS.replace(
            r#"<event name="A"><text>second"#,
            r#"<event name="A" seen="1"><text>second"#
        ))
    );
    let patch = r#"<mod:findName type="event" name="A" reverse="false" limit="-1"><mod:removeTag/></mod:findName>"#;
    let out = append(EVENTS, patch).unwrap();
    assert!(!out.iter().any(|l| l.contains("\"A\"")), "{out:#?}");
}

#[test]
fn find_like_and_find_with_child_like_use_selectors() {
    let patch = r#"<mod:findLike type="event"><mod:selector unique="true"/><mod:removeAttributes unique=""/><mod:setValue>replaced</mod:setValue></mod:findLike>"#;
    assert_eq!(
        append(EVENTS, patch).unwrap(),
        normalized(&EVENTS.replace(
            r#"<event name="B" unique="true"><text>b</text><choice><text>ok</text></choice></event>"#,
            r#"<event name="B">replaced</event>"#
        ))
    );
    let patch = r#"<mod:findWithChildLike type="event" child-type="text"><mod:selector>second</mod:selector><mod:setAttributes found="yes"/></mod:findWithChildLike>"#;
    let out = append(EVENTS, patch).unwrap();
    assert_eq!(out.iter().filter(|l| l.contains("found")).count(), 1);
    assert!(
        out.iter()
            .any(|l| l.trim() == "<event found=\"yes\" name=\"A\">"),
        "{out:#?}"
    );
}

#[test]
fn appends_overwrites_and_nested_finds_change_what_was_found() {
    let patch = r#"<mod:findName type="event" name="B">
        <mod-overwrite:text>new</mod-overwrite:text>
        <mod-append:item id="1"/>
        <mod-overwrite:missing/>
        <mod:findLike type="choice"><mod:findLike type="text"><mod:setValue>changed</mod:setValue></mod:findLike></mod:findLike>
    </mod:findName>"#;
    assert_eq!(
        append(EVENTS, patch).unwrap(),
        normalized(&EVENTS.replace(
            r#"<text>b</text><choice><text>ok</text></choice>"#,
            r#"<text>new</text><choice><text>changed</text></choice><item id="1"/><missing/>"#
        ))
    );
}

#[test]
fn composite_finds_combine_their_parts() {
    let or = r#"<mod:findComposite><mod:par op="OR">
        <mod:findName type="event" name="B"/>
        <mod:findLike type="ship"/>
    </mod:par><mod:setAttributes hit="1"/></mod:findComposite>"#;
    let out = append(EVENTS, or).unwrap();
    assert_eq!(
        out.iter().filter(|l| l.contains("hit")).count(),
        2,
        "{out:#?}"
    );
    let and = r#"<mod:findComposite><mod:par op="AND">
        <mod:findLike type="event"/>
        <mod:findLike><mod:selector unique="true"/></mod:findLike>
    </mod:par><mod:setAttributes hit="1"/></mod:findComposite>"#;
    let out = append(EVENTS, and).unwrap();
    assert_eq!(
        out.iter()
            .filter(|l| l.contains("hit"))
            .map(|l| l.trim())
            .collect::<Vec<_>>(),
        ["<event hit=\"1\" name=\"B\" unique=\"true\">"]
    );
}

#[test]
fn panicking_finds_fail_when_nothing_matches() {
    let quiet = r#"<mod:findName type="event" name="nope"><mod:removeTag/></mod:findName>"#;
    assert_eq!(append(EVENTS, quiet).unwrap(), normalized(EVENTS));
    let loud = r#"<mod:findName type="event" name="nope" panic="true"/>"#;
    assert!(append(EVENTS, loud).unwrap_err().contains("found nothing"));
    let custom = r#"<mod:findLike type="nope" panic="needs mod X"/>"#;
    assert_eq!(append(EVENTS, custom).unwrap_err(), "needs mod X");
    assert!(append(EVENTS, r#"<mod:findName type="event"/>"#).is_err());
}

#[test]
fn unknown_commands_are_warned_about() {
    let mut doc = Doc::parse(EVENTS);
    let mut warnings = Vec::new();
    doc.append(
        r#"<mod:findName name="S"><mod:frobnicate/></mod:findName>"#,
        &mut warnings,
    )
    .unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("mod:frobnicate"));
}

#[test]
fn raw_appends_keep_the_root_around_both() {
    assert_eq!(
        patch::raw_append(
            "<?xml version=\"1.0\"?>\n<FTL><a/></FTL>",
            "<FTL><b/></FTL>"
        ),
        "<FTL><a/>\n<b/>\n</FTL>\n"
    );
    assert_eq!(patch::raw_append("<a/>", "<b/>"), "<a/>\n<b/>\n");
}

#[test]
fn line_endings_become_crlf() {
    assert_eq!(crlf(b"a\nb\r\nc\rd"), b"a\r\nb\r\nc\r\nd");
}

#[test]
fn element_helpers() {
    let mut e = Element::new("x");
    e.set_attr("a", "1");
    e.set_attr("a", "2");
    e.set_attr("b", "3");
    e.remove_attr("b");
    assert_eq!(e.attrs, [("a".to_owned(), "2".to_owned())]);
}

mod deploying {
    use patchwork_core::ext::testing::TestGame;

    use super::*;

    /// The bytes of an archive of `files`.
    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ftl.dat");
        let files: Vec<_> = files
            .iter()
            .map(|(p, c)| (p.to_string(), Data::Bytes(c.as_bytes().to_vec())))
            .collect();
        pkg::write(&path, None, &files).unwrap();
        fs::read(path).unwrap()
    }

    /// The files in the archive at `path`, by path.
    fn contents(path: &Path) -> BTreeMap<String, String> {
        let mut file = File::open(path).unwrap();
        let index = pkg::read_index(&mut BufReader::new(&mut file)).unwrap();
        index
            .into_iter()
            .map(|e| {
                let bytes = pkg::read_entry(&mut file, e.offset, e.size).unwrap();
                (e.path, String::from_utf8(bytes).unwrap())
            })
            .collect()
    }

    const BLUEPRINTS: &str =
        "<FTL><weaponBlueprint name=\"A\"><damage>1</damage></weaponBlueprint></FTL>";

    fn game() -> TestGame {
        TestGame::builtin("ftl").extension(Ftl).game_file(
            "data/ftl.dat",
            archive(&[
                ("data/blueprints.xml", BLUEPRINTS),
                ("img/a.png", "old"),
                ("data/text.xml", "<FTL></FTL>"),
            ]),
        )
    }

    /// A zip of `files`.
    fn zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (path, contents) in files {
            zip.start_file(*path, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn mods_are_patched_into_the_archive_in_load_order() {
        let t = game()
            .mod_files(
                "First",
                [
                    (
                        "data/blueprints.xml.append",
                        "<mod:findName type=\"weaponBlueprint\" name=\"A\"><mod-overwrite:damage>2</mod-overwrite:damage></mod:findName>",
                    ),
                    ("IMG/A.png", "new"),
                    ("mod-appendix/metadata.xml", "<metadata/>"),
                    ("data/layout.txt", "a\nb"),
                    ("data/text.xml.rawappend", "<FTL><text name=\"x\">raw</text></FTL>"),
                ],
            )
            .mod_files(
                "Second",
                [(
                    "Second v1.0/data/blueprints.xml.append",
                    "<mod:findName type=\"weaponBlueprint\" name=\"A\"><mod-append:extra/></mod:findName><weaponBlueprint name=\"B\"/>",
                )],
            );
        let plan = t.deploy().unwrap();
        assert_eq!(plan.placed(), ["data/ftl.dat"]);
        assert_eq!(plan.placed_by("data/ftl.dat"), Some(BUILT_BY));
        assert!(plan.warnings().is_empty(), "{:?}", plan.warnings());

        let files = contents(&t.path("data/ftl.dat"));
        assert_eq!(
            files.keys().collect::<Vec<_>>(),
            [
                "data/blueprints.xml",
                "data/layout.txt",
                "data/text.xml",
                "img/a.png"
            ]
        );
        assert_eq!(
            files["data/blueprints.xml"],
            "<FTL><weaponBlueprint name=\"A\"><damage>2</damage><extra /></weaponBlueprint><weaponBlueprint name=\"B\" /></FTL>"
        );
        assert_eq!(files["img/a.png"], "new");
        assert_eq!(files["data/layout.txt"], "a\r\nb");
        assert_eq!(
            files["data/text.xml"],
            "<FTL>\n<text name=\"x\">raw</text>\n</FTL>\n"
        );

        // Taking every mod out puts the game's own archive back.
        t.set_enabled("First", false).unwrap();
        t.set_enabled("Second", false).unwrap();
        t.deploy().unwrap();
        assert_eq!(
            contents(&t.path("data/ftl.dat"))["data/blueprints.xml"],
            BLUEPRINTS
        );
    }

    #[test]
    fn ftl_files_in_a_mod_are_opened() {
        let hyperspace = zip(&[
            ("data/hyperspace.xml", "<FTL><version>1</version></FTL>"),
            ("mod-appendix/metadata.xml", "<metadata/>"),
        ]);
        let t = game().mod_files(
            "Hyperspace",
            [
                ("Hyperspace.ftl", hyperspace),
                ("README.txt", b"read me".to_vec()),
                ("Linux/Hyperspace.1.6.13.amd64.so", b"elf".to_vec()),
            ],
        );
        let plan = t.deploy().unwrap();
        assert!(plan.warnings().is_empty(), "{:?}", plan.warnings());
        let files = contents(&t.path("data/ftl.dat"));
        assert_eq!(
            files["data/hyperspace.xml"],
            "<FTL><version>1</version></FTL>"
        );
        assert!(
            !files
                .keys()
                .any(|k| k.contains("README") || k.contains("Linux"))
        );
    }

    #[test]
    fn mods_are_applied_to_the_unpatched_archive_ftlman_kept() {
        let t = game()
            .game_file(
                "data/ftl.dat.vanilla",
                archive(&[("data/blueprints.xml", "<FTL></FTL>")]),
            )
            .mod_files("Mod", [("data/blueprints.xml.append", "<b/>")]);
        t.deploy().unwrap();
        let files = contents(&t.path("data/ftl.dat"));
        assert_eq!(files.len(), 1);
        assert_eq!(files["data/blueprints.xml"], "<FTL><b /></FTL>");
    }

    #[test]
    fn problems_are_reported() {
        let t = game().mod_files("Loose", [("readme.txt", "hi")]).mod_files(
            "Broken",
            [(
                "data/blueprints.xml.append",
                "<mod:findName type=\"weaponBlueprint\" name=\"nope\" panic=\"true\"/>",
            )],
        );
        let err = t.deploy().unwrap_err().to_string();
        assert!(err.contains("Broken: data/blueprints.xml.append"), "{err}");

        t.set_enabled("Broken", false).unwrap();
        let plan = t.deploy().unwrap();
        assert!(
            plan.warnings()
                .iter()
                .any(|w| w.starts_with("Loose has no data/, audio/, fonts/ or img/ folder")),
            "{:?}",
            plan.warnings()
        );

        let t = TestGame::builtin("ftl")
            .extension(Ftl)
            .mod_files("Mod", [("data/a.xml", "<a/>")]);
        assert!(
            t.deploy()
                .unwrap_err()
                .to_string()
                .contains("ftl.dat wasn't found")
        );
    }

    const SCRIPT: &str = "#!/bin/sh\nhere=\"$(dirname \"$0\")\"\nexec \"$here/FTL.amd64\" \"$@\"\n";

    /// Hyperspace's Linux and Windows release zips, as unpacked.
    fn hyperspace(platform: &str) -> Vec<(&'static str, Vec<u8>)> {
        let ftl = zip(&[("data/hyperspace.xml", "<FTL/>")]);
        let mut files = vec![("Hyperspace.ftl", ftl), ("README.txt", b"hi".to_vec())];
        match platform {
            "Linux" => files.extend([
                ("Linux/Hyperspace.1.6.12.amd64.so", b"gog".to_vec()),
                ("Linux/Hyperspace.1.6.13.amd64.so", b"steam".to_vec()),
            ]),
            _ => files.extend([
                (
                    "Windows - Extract these files into where FTLGame.exe is/Hyperspace.dll",
                    b"hs".to_vec(),
                ),
                (
                    "Windows - Extract these files into where FTLGame.exe is/xinput1_4.dll",
                    b"proxy".to_vec(),
                ),
            ]),
        }
        files
    }

    fn linux_game() -> TestGame {
        game()
            .game_file("data/FTL.amd64", "elf")
            .game_file("data/FTL", SCRIPT)
    }

    #[test]
    fn hyperspace_for_steams_linux_build_is_preloaded() {
        let t = linux_game().mod_files("Hyperspace", hyperspace("Linux"));
        File::options()
            .write(true)
            .open(t.path("data/FTL.amd64"))
            .unwrap()
            .set_len(STEAM_LINUX_SIZE)
            .unwrap();
        let plan = t.deploy().unwrap();
        assert!(plan.warnings().is_empty(), "{:?}", plan.warnings());
        assert_eq!(
            plan.placed(),
            [
                "data/FTL",
                "data/Hyperspace.1.6.13.amd64.so",
                "data/ftl.dat"
            ]
        );
        assert_eq!(plan.placed_by("data/FTL"), Some("Hyperspace"));
        assert_eq!(t.read("data/Hyperspace.1.6.13.amd64.so").unwrap(), "steam");
        let script = t.read("data/FTL").unwrap();
        let preload = script.find("export LD_PRELOAD=").unwrap();
        assert!(preload < script.find("exec ").unwrap(), "{script}");
        assert!(
            script.contains("LD_PRELOAD=\"Hyperspace.1.6.13.amd64.so"),
            "{script}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(t.path("data/FTL"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }

        // Taking it out puts the game's script back.
        t.set_enabled("Hyperspace", false).unwrap();
        t.deploy().unwrap();
        assert_eq!(t.read("data/FTL").unwrap(), SCRIPT);
        assert!(!t.exists("data/Hyperspace.1.6.13.amd64.so"));
    }

    #[test]
    fn hyperspace_for_other_linux_builds_is_1_6_12() {
        let t = linux_game().mod_files("Hyperspace", hyperspace("Linux"));
        let plan = t.deploy().unwrap();
        assert!(plan.places("data/Hyperspace.1.6.12.amd64.so"));
        assert!(!plan.places("data/Hyperspace.1.6.13.amd64.so"));
        assert!(t.read("data/FTL").unwrap().contains("1.6.12"));
    }

    #[test]
    fn hyperspace_dlls_go_next_to_the_windows_exe() {
        let t = game()
            .game_file("FTLGame.exe", "exe")
            .mod_files("Hyperspace", hyperspace("Windows"));
        let plan = t.deploy().unwrap();
        assert!(plan.warnings().is_empty(), "{:?}", plan.warnings());
        assert_eq!(
            plan.placed(),
            ["Hyperspace.dll", "data/ftl.dat", "xinput1_4.dll"]
        );
    }

    #[test]
    fn code_for_another_platform_is_warned_about() {
        let t = game()
            .game_file("FTLGame.exe", "exe")
            .mod_files("Hyperspace", hyperspace("Linux"));
        let plan = t.deploy().unwrap();
        assert_eq!(plan.placed(), ["data/ftl.dat"]);
        assert!(
            plan.warnings()
                .iter()
                .any(|w| w.starts_with("Hyperspace has code for Linux")),
            "{:?}",
            plan.warnings()
        );

        let t = linux_game().mod_files("Hyperspace", [("Hyperspace.1.6.9.amd64.so", "old")]);
        let plan = t.deploy().unwrap();
        assert!(!plan.places("data/FTL"));
        assert!(
            plan.warnings()
                .iter()
                .any(|w| w.contains("Hyperspace for FTL 1.6.9, but this is FTL 1.6.12")),
            "{:?}",
            plan.warnings()
        );
    }

    #[test]
    fn scripts_get_the_preload_before_their_exec() {
        assert_eq!(
            hyperspace_so_version("Hyperspace.1.6.13.amd64.so"),
            Some("1.6.13")
        );
        assert_eq!(hyperspace_so_version("liblua5.3.so.0"), None);
        assert_eq!(preloading_script("#!/bin/sh\necho hi\n", "x.so"), None);
        let script = preloading_script("a\nexec b\n# exec\n", "x.so").unwrap();
        assert!(script.starts_with("a\n# Added by patchwork"), "{script}");
        assert!(script.ends_with("\nexec b\n# exec\n"), "{script}");
    }
}
