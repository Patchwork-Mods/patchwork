use patchwork_core::deploy::{self, DeployState, ResolvedRoots, Roots};
use patchwork_core::event::Silent;
use patchwork_core::game::{DeployerSpec, GameDef};
use patchwork_core::registry::Registry;
use rpkg_rs::resource::package_builder::{PackageBuilder, PackageResourceBuilder};
use rpkg_rs::resource::resource_package::{
    PackageVersion, ResourcePackage, ResourceReferenceFlags, ResourceReferenceFlagsStandard,
};
use std::path::Path;

use glacier_texture::GlacierGame;

use super::resource::COMPRESSION_LEVEL;
use super::*;

const PACKAGE_DEFINITION_TEXT: &str = "// game partitions\r\n\
@partition name=chunk0 parent=none type=standard patchlevel=3\r\n\
[assembly:/_pro/scenes/boot.entity].entitytemplate\r\n\
@partition name=chunk1 parent=chunk0 type=standard patchlevel=12\r\n";
const THUMBS_TEXT: &str = "[application]\nForceVSync=1\n[Hitman5]\nPrimaryScene=boot\n";

// Resources in the fake game's archives.
const IN_CHUNK0: u64 = 0x00A0_0000_0000_0001;
const IN_CHUNK1: u64 = 0x00A1_0000_0000_0001;
const ROOT: u64 = 0x00A2_0000_0000_0001;
const IN_CHUNK2: u64 = 0x00A2_0000_0000_0002;
const UNREFERENCED: u64 = 0x00A2_0000_0000_0003;
const GAME_CONTRACT: u64 = 0x00AA_0000_0000_0001;

const UNLOCKABLES_JSON: &str = r#"[{"Id":"GUN","Properties":{"Name":"gun","RepositoryId":"r1"}}]"#;
const REPOSITORY_JSON: &str = r#"[{"ID_":"r1","Name":"Gun","Tags":["a","b"]}]"#;
const DESTINATIONS_JSON: &str = r#"{"Root":{"Children":[{"Id":"m1"},{"Id":"m2"}]}}"#;

struct Env {
    tmp: tempfile::TempDir,
    game: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        fs::create_dir_all(game.join("Runtime")).unwrap();
        fs::create_dir_all(game.join("Retail")).unwrap();
        fs::write(
            game.join(PACKAGE_DEFINITION),
            text::encrypt(PACKAGE_DEFINITION_TEXT.to_owned()).unwrap(),
        )
        .unwrap();
        fs::write(
            game.join(THUMBS),
            text::encrypt(THUMBS_TEXT.to_owned()).unwrap(),
        )
        .unwrap();
        let env = Self { tmp, game };
        env.archive(
            "chunk0",
            PatchId::Base,
            &[
                (IN_CHUNK0, "TEMP", b"zero".to_vec(), &[]),
                (
                    json::UNLOCKABLES,
                    "ORES",
                    ores::Ores::Json(UNLOCKABLES_JSON.to_owned()).serialize(),
                    &[IN_CHUNK1],
                ),
                (
                    json::REPOSITORY,
                    "REPO",
                    REPOSITORY_JSON.as_bytes().to_vec(),
                    &[IN_CHUNK1],
                ),
                (
                    json::CONTRACTS,
                    "ORES",
                    ores::Ores::Hashes(vec![(GAME_CONTRACT, "game-contract".to_owned())])
                        .serialize(),
                    &[GAME_CONTRACT],
                ),
                (
                    GAME_CONTRACT,
                    "JSON",
                    br#"{"Metadata":{"Id":"game-contract"}}"#.to_vec(),
                    &[IN_CHUNK1],
                ),
                (
                    json::BLOBS,
                    "ORES",
                    ores::Ores::Hashes(vec![(0x00BB_0000_0000_0001, "images/old.jpg".to_owned())])
                        .serialize(),
                    &[],
                ),
                (
                    json::DESTINATIONS,
                    "JSON",
                    DESTINATIONS_JSON.as_bytes().to_vec(),
                    &[IN_CHUNK1],
                ),
            ],
        );
        env.archive(
            "chunk1",
            PatchId::Base,
            &[(IN_CHUNK1, "TEMP", b"one".to_vec(), &[])],
        );
        env.archive(
            "chunk2",
            PatchId::Base,
            &[
                (
                    ROOT,
                    "TEMP",
                    b"root".to_vec(),
                    &[IN_CHUNK0, IN_CHUNK1, IN_CHUNK2],
                ),
                (IN_CHUNK2, "TEMP", b"old two".to_vec(), &[]),
                (UNREFERENCED, "TEMP", b"other".to_vec(), &[]),
            ],
        );
        env.archive(
            "chunk2",
            PatchId::Patch(1),
            &[(IN_CHUNK2, "TEMP", b"two".to_vec(), &[])],
        );
        env
    }

    fn archive(&self, partition: &str, patch: PatchId, resources: &[(u64, &str, Vec<u8>, &[u64])]) {
        let id = PartitionId::from_str(partition).unwrap();
        let mut builder = PackageBuilder::new_with_patch_id(id.clone(), patch);
        for (hash, resource_type, data, references) in resources {
            let mut resource = PackageResourceBuilder::from_memory(
                RuntimeResourceID::from(*hash),
                resource_type,
                data.clone(),
                Some(COMPRESSION_LEVEL),
                true,
            )
            .unwrap();
            for reference in *references {
                resource.with_reference(
                    RuntimeResourceID::from(*reference),
                    ResourceReferenceFlags::Standard(ResourceReferenceFlagsStandard::from_bits(
                        0x1F,
                    )),
                );
            }
            builder.with_resource(resource);
        }
        let path = self.game.join("Runtime").join(id.to_filename(patch));
        builder.build_to_file(PackageVersion::RPKGv2, path).unwrap();
    }

    /// A mod made of `files`, as (path, contents).
    fn installed(&self, name: &str, files: &[(&str, &[u8])]) -> InstalledMod {
        let dir = self.tmp.path().join("mods").join(name);
        let files = files
            .iter()
            .map(|(rel, contents)| {
                let src = RelPath::new(rel).unwrap().under(&dir);
                fs::create_dir_all(src.parent().unwrap()).unwrap();
                fs::write(&src, contents).unwrap();
                ModFile {
                    rel: RelPath::new(rel).unwrap(),
                    src,
                }
            })
            .collect();
        InstalledMod {
            name: name.to_owned(),
            source: "local".to_owned(),
            id: name.to_owned(),
            version: None,
            hash: ContentHash::of(name.as_bytes()),
            files,
        }
    }

    fn build(&self, mods: &[InstalledMod]) -> Result<DesiredState> {
        let def = GameDef {
            schema: 1,
            id: "hitman-woa".to_owned(),
            name: "HITMAN World of Assassination".to_owned(),
            description: None,
            image: None,
            hidden: false,
            sources: Default::default(),
            deployer: DeployerSpec {
                kind: "smf".to_owned(),
                config: toml::Table::new(),
            },
            link: Default::default(),
            detect: Default::default(),
            versions: Default::default(),
            roots: Default::default(),
            launch: Default::default(),
            fomod: None,
        };
        let registry = Registry::default();
        let ctx = DeployContext {
            game: &def,
            config: &def.deployer.config,
            registry: &registry,
        };
        let state = DeployState::empty("hitman-woa", &Roots::game(&self.game));
        let out = self.tmp.path().join("out");
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(&out).unwrap();
        let cache = self.tmp.path().join("cache");
        let build = BuildContext {
            game: GameFiles::new(&self.game, &state),
            out: &out,
            cache: &cache,
            reporter: &Silent,
            load_order: None,
        };
        SmfDeployer.build(&ctx, &build, mods)
    }
}

/// Where the file placed at `dst` is, checked the way a deploy would.
fn placed(env: &Env, desired: &DesiredState, dst: &str) -> PathBuf {
    let out = env.tmp.path().join("out");
    let roots = ResolvedRoots::game(&env.game);
    deploy::resolve("smf", desired.placements.clone(), &[], Some(&out), &roots)
        .unwrap()
        .into_iter()
        .find(|p| p.dst.to_string() == dst)
        .unwrap_or_else(|| panic!("nothing placed at {dst}"))
        .src
}

/// Each resource in an archive, with its data and references.
fn contents(path: &Path) -> BTreeMap<u64, (Vec<u8>, Vec<u64>)> {
    let package = ResourcePackage::from_file(path).unwrap();
    package
        .resources()
        .iter()
        .map(|(rrid, info)| {
            let references = info
                .references()
                .iter()
                .map(|(r, _)| u64::from(*r))
                .collect();
            let data = package.read_resource(rrid).unwrap();
            (u64::from(*rrid), (data, references))
        })
        .collect()
}

fn decrypted(path: &Path) -> String {
    text::decrypt(&fs::read(path).unwrap()).unwrap()
}

const MANIFEST: &str = r#"{
    // JSON5, like SMF reads it
    id: "Someone.Mod",
    name: "A Mod",
    contentFolders: ["content"],
    thumbs: ["ConsoleCmd UI_EnableMouseEvents 0"],
    packagedefinition: [
        { type: "partition", name: "mymod", parent: "chunk0", partitionType: "standard" },
        { type: "entity", partition: "chunk0", path: "[assembly:/_pro/my.entity].entitytemplate" },
    ],
    options: [
        { name: "On", type: "checkbox", enabledByDefault: true, contentFolders: ["on"] },
        { name: "Off", type: "checkbox", contentFolders: ["off"] },
    ],
}"#;

const META_JSON: &str = r#"{
    "hash_value": "00B0000000000001",
    "hash_offset": 0,
    "hash_size": 2147483648,
    "hash_resource_type": "TEMP",
    "hash_reference_table_size": 13,
    "hash_reference_table_dummy": 0,
    "hash_size_final": 4,
    "hash_size_in_memory": 4294967295,
    "hash_size_in_video_memory": 4294967295,
    "hash_reference_data": [{ "hash": "00A0000000000001", "flag": "1F" }]
}"#;

#[test]
fn framework_mods_are_built_into_a_patch_archive() {
    let env = Env::new();
    let first = env.installed(
        "first",
        &[
            ("Mod/manifest.json", MANIFEST.as_bytes()),
            ("Mod/content/chunk0/00B0000000000001.TEMP", b"temp"),
            (
                "Mod/content/chunk0/00B0000000000001.TEMP.meta.json",
                META_JSON.as_bytes(),
            ),
            ("Mod/content/chunk0/sub/00B0000000000002.TBLU", b"first"),
            ("Mod/on/chunk0/00B0000000000003.TBLU", b"on"),
            ("Mod/off/chunk0/00B0000000000004.TBLU", b"off"),
            ("Mod/content/chunk0/readme.txt", b"ignored"),
        ],
    );
    let second = env.installed(
        "second",
        &[
            (
                "manifest.json",
                br#"{ "id": "Someone.Other", "name": "Other", "contentFolders": ["Content"],
                      "requirements": ["Someone.Missing"] }"#,
            ),
            ("content/chunk0/00B0000000000002.TBLU", b"second"),
        ],
    );
    let desired = env.build(&[first, second]).unwrap();

    let archive = contents(&placed(&env, &desired, "Runtime/chunk0patch300.rpkg"));
    let data = |hash: u64| archive.get(&hash).map(|(d, _)| d.as_slice());
    assert_eq!(data(0x00B0_0000_0000_0001), Some(&b"temp"[..]));
    assert_eq!(archive[&0x00B0_0000_0000_0001].1, vec![IN_CHUNK0]);
    // later mods win, and nested folders count
    assert_eq!(data(0x00B0_0000_0000_0002), Some(&b"second"[..]));
    // options enabled by default are used, others aren't
    assert_eq!(data(0x00B0_0000_0000_0003), Some(&b"on"[..]));
    assert_eq!(data(0x00B0_0000_0000_0004), None);
    assert_eq!(archive.len(), 3);

    let pd = decrypted(&placed(&env, &desired, PACKAGE_DEFINITION));
    assert!(
        !pd.contains("patchlevel=3 ") && !pd.contains("patchlevel=12"),
        "{pd}"
    );
    assert!(
        pd.contains(
            "@partition name=chunk0 parent=none type=standard patchlevel=310\r\n\
         [assembly:/_pro/my.entity].entitytemplate\r\n\
         [assembly:/_pro/scenes/boot.entity].entitytemplate"
        ),
        "{pd}"
    );
    assert!(pd.contains("@partition name=mymod parent=chunk0 type=standard patchlevel=310"));

    let thumbs = decrypted(&placed(&env, &desired, THUMBS));
    assert_eq!(
        thumbs,
        "[application]\nForceVSync=1\n[Hitman5]\nConsoleCmd UI_EnableMouseEvents 0\nPrimaryScene=boot"
    );
    assert!(
        desired
            .warnings
            .iter()
            .any(|w| w.contains("Someone.Missing")),
        "{:?}",
        desired.warnings
    );
}

#[test]
fn rpkg_mods_and_dependencies_are_staged() {
    let env = Env::new();
    let package = {
        let mut builder = PackageBuilder::new_with_patch_id(
            PartitionId::from_str("chunk0").unwrap(),
            PatchId::Patch(5),
        );
        builder.with_resource(
            PackageResourceBuilder::from_memory(
                RuntimeResourceID::from(0x00C0_0000_0000_0001),
                "TEXT",
                b"plain".to_vec(),
                None,
                false,
            )
            .unwrap(),
        );
        builder.with_resource(
            PackageResourceBuilder::from_memory(
                RuntimeResourceID::from(IN_CHUNK2),
                "TEMP",
                b"from the mod".to_vec(),
                Some(COMPRESSION_LEVEL),
                true,
            )
            .unwrap(),
        );
        builder.build_to_vec(PackageVersion::RPKGv2).unwrap()
    };
    let rpkg = env.installed("rpkg", &[("chunk0/my mod.rpkg", &package)]);
    let other = env.installed("named", &[("chunk2patch7.rpkg", &package)]);
    let deps = env.installed(
        "deps",
        &[(
            "manifest.json",
            format!(r#"{{ "id": "Deps", "name": "Deps", "dependencies": ["{ROOT:016X}"] }}"#)
                .as_bytes(),
        )],
    );
    let desired = env.build(&[rpkg, other, deps]).unwrap();

    let chunk0 = contents(&placed(&env, &desired, "Runtime/chunk0patch300.rpkg"));
    let data = |hash: u64| chunk0.get(&hash).map(|(d, _)| d.as_slice());
    assert_eq!(data(0x00C0_0000_0000_0001), Some(&b"plain"[..]));
    // dependencies don't replace what mods staged
    assert_eq!(data(IN_CHUNK2), Some(&b"from the mod"[..]));
    assert_eq!(data(ROOT), Some(&b"root"[..]));
    assert_eq!(chunk0[&ROOT].1, vec![IN_CHUNK0, IN_CHUNK1, IN_CHUNK2]);
    // only what's outside chunks 0 and 1, and only what's referenced
    assert_eq!(data(IN_CHUNK0), None);
    assert_eq!(data(IN_CHUNK1), None);
    assert_eq!(data(UNREFERENCED), None);

    // an archive named after its partition goes there
    let chunk2 = contents(&placed(&env, &desired, "Runtime/chunk2patch300.rpkg"));
    assert_eq!(chunk2.len(), 2);
}

#[test]
fn dependencies_take_the_latest_patch_and_can_include_chunk1() {
    let env = Env::new();
    let deps = env.installed(
        "deps",
        &[(
            "manifest.json",
            format!(
                r#"{{ "id": "Deps", "name": "Deps",
                      "dependencies": [{{ "runtimeID": "{ROOT:016X}", "toChunk": 1, "portFromChunk1": true }}] }}"#
            )
            .as_bytes(),
        )],
    );
    let desired = env.build(&[deps]).unwrap();
    let chunk1 = contents(&placed(&env, &desired, "Runtime/chunk1patch300.rpkg"));
    let data = |hash: u64| chunk1.get(&hash).map(|(d, _)| d.as_slice());
    assert_eq!(data(IN_CHUNK2), Some(&b"two"[..]));
    assert_eq!(data(IN_CHUNK1), Some(&b"one"[..]));
    assert_eq!(data(IN_CHUNK0), None);
}

#[test]
fn new_partitions_get_a_base_archive() {
    let env = Env::new();
    let m = env.installed(
        "m",
        &[
            (
                "manifest.json",
                br#"{ id: "M", name: "M", contentFolders: ["c"] }"#,
            ),
            ("c/chunk40/00D0000000000001.TEMP", b"new"),
        ],
    );
    let desired = env.build(&[m]).unwrap();
    placed(&env, &desired, "Runtime/chunk40.rpkg");
}

#[test]
fn mods_needing_unsupported_features_fail() {
    let env = Env::new();
    let m = env.installed(
        "m",
        &[
            (
                "manifest.json",
                br#"{ id: "M", name: "M", contentFolders: ["c"],
                      localisation: { english: { UI_X: "x" } } }"#,
            ),
            ("c/chunk0/thing.material.json", b"{}"),
        ],
    );
    let err = env.build(&[m]).unwrap_err().to_string();
    assert!(err.contains(".material.json"), "{err}");
    assert!(err.contains("localisation"), "{err}");
}

#[test]
fn nothing_to_deploy_leaves_the_game_alone() {
    let env = Env::new();
    let readme = env.installed("readme", &[("readme.txt", b"hi")]);
    let desired = env.build(&[readme]).unwrap();
    assert!(desired.placements.is_empty());
    assert_eq!(desired.warnings.len(), 1);
}

#[test]
fn binary_meta_matches_json_meta() {
    let mut binary = Vec::new();
    binary.extend_from_slice(&0x00B0_0000_0000_0001u64.to_le_bytes());
    binary.extend_from_slice(&0u64.to_le_bytes());
    binary.extend_from_slice(&0x8000_0000u32.to_le_bytes());
    binary.extend_from_slice(b"TEMP");
    binary.extend_from_slice(&13u32.to_le_bytes());
    binary.extend_from_slice(&0u32.to_le_bytes());
    binary.extend_from_slice(&4u32.to_le_bytes());
    binary.extend_from_slice(&u32::MAX.to_le_bytes());
    binary.extend_from_slice(&u32::MAX.to_le_bytes());
    binary.extend_from_slice(&(1u32 | 0xC000_0000).to_le_bytes());
    binary.push(0x1F);
    binary.extend_from_slice(&IN_CHUNK0.to_le_bytes());

    let from_binary = Meta::from_binary(&binary).unwrap();
    assert_eq!(from_binary, Meta::from_json(META_JSON).unwrap());
    assert!(from_binary.scrambled && !from_binary.compressed);
    assert_eq!(from_binary.system_memory(100), u32::MAX);
}

#[test]
fn memory_needs_scale_with_the_resource() {
    let json = META_JSON.replace(
        r#""hash_size_in_memory": 4294967295"#,
        r#""hash_size_in_memory": 8"#,
    );
    // twice the size of the 4 byte original
    assert_eq!(Meta::from_json(&json).unwrap().system_memory(10), 20);
}

#[test]
fn resource_paths_hash_like_rpkg_tool() {
    let path = "[assembly:/_pro/scenes/boot.entity].entitytemplate";
    assert_eq!(
        meta::parse_hash(path),
        RuntimeResourceID::from_raw_string(path)
    );
    assert_eq!(
        meta::parse_hash(&path.to_uppercase()),
        RuntimeResourceID::from_raw_string(path)
    );
    assert_eq!(u64::from(meta::parse_hash("00A0000000000001")), IN_CHUNK0);
}

#[test]
fn archive_names() {
    let name = ArchiveName::parse("chunk2patch7.rpkg").unwrap();
    assert_eq!(name.partition.to_string(), "chunk2");
    assert_eq!(name.patch, Some(7));
    assert_eq!(ArchiveName::parse("chunk0.rpkg").unwrap().patch, None);
    assert!(ArchiveName::parse("packagedefinition.txt").is_none());

    let partition = |p: &str| archive_partition(&RelPath::new(p).unwrap());
    assert_eq!(partition("Mod/chunk3/anything.rpkg"), "chunk3");
    assert_eq!(partition("dlc5patch2.rpkg"), "dlc5");
    assert_eq!(partition("cool mod.rpkg"), "chunk0");
}

/// A resource in a built archive, with its header.
fn resource(archive: &Path, hash: u64) -> resource::Loaded {
    let package = read_package(archive).unwrap();
    let info = package
        .resources()
        .get(&RuntimeResourceID::from(hash))
        .unwrap_or_else(|| panic!("{hash:016X} isn't in {}", archive.display()));
    resource::load_packaged(&package, info).unwrap()
}

fn references(loaded: &resource::Loaded) -> Vec<(u64, u8)> {
    loaded
        .header
        .references
        .iter()
        .map(|(r, f)| (u64::from(*r), *f))
        .collect()
}

fn json_of(data: &[u8]) -> Value {
    json::parse(data).unwrap()
}

fn ores_json(data: &[u8]) -> Value {
    match ores::Ores::parse(data).unwrap() {
        ores::Ores::Json(text) => json_of(text.as_bytes()),
        other => panic!("not a JSON ORES: {other:?}"),
    }
}

fn ores_hashes(data: &[u8]) -> Vec<(u64, String)> {
    match ores::Ores::parse(data).unwrap() {
        ores::Ores::Hashes(entries) => entries,
        other => panic!("not a hash ORES: {other:?}"),
    }
}

/// A framework mod with one content folder holding `files` in chunk0.
fn content_mod(env: &Env, name: &str, files: &[(&str, &str)]) -> InstalledMod {
    let manifest = format!(r#"{{ id: "{name}", name: "{name}", contentFolders: ["content"] }}"#);
    let mut all = vec![("manifest.json".to_owned(), manifest)];
    for (file, contents) in files {
        all.push((format!("content/chunk0/{file}"), (*contents).to_owned()));
    }
    let borrowed: Vec<(&str, &[u8])> = all
        .iter()
        .map(|(f, c)| (f.as_str(), c.as_bytes()))
        .collect();
    env.installed(name, &borrowed)
}

#[test]
fn unlockables_and_repository_edits_build_on_each_other() {
    let env = Env::new();
    let first = content_mod(
        &env,
        "First",
        &[
            (
                "guns.unlockables.json",
                r#"{ "GUN": { "Properties": { "Name": "better gun" } }, "NEW": { "Type": "weapon" } }"#,
            ),
            (
                "items.repository.json",
                r#"{ "r1": { "Name": "Gun 2", "Tags": ["c"] },
                     "r2": { "Name": "New", "Runtime": "12345", "Image": "images/New.jpg" } }"#,
            ),
        ],
    );
    let second = content_mod(
        &env,
        "Second",
        &[(
            "more.unlockables.json",
            r#"{ "NEW": { "Properties": { "Damage": 2 } } }"#,
        )],
    );
    let desired = env.build(&[first, second]).unwrap();
    let archive = placed(&env, &desired, "Runtime/chunk0patch300.rpkg");

    let unlockables = resource(&archive, json::UNLOCKABLES);
    assert_eq!(
        ores_json(&unlockables.data),
        serde_json::json!([
            { "Id": "GUN", "Properties": { "Name": "better gun", "RepositoryId": "r1" } },
            { "Type": "weapon", "Properties": { "Damage": 2 }, "Id": "NEW" },
        ])
    );
    // the game's header is kept
    assert_eq!(unlockables.resource_type, "ORES");
    assert_eq!(references(&unlockables), vec![(IN_CHUNK1, 0x1F)]);

    let repository = resource(&archive, json::REPOSITORY);
    assert_eq!(
        json_of(&repository.data),
        serde_json::json!([
            // arrays are replaced rather than merged
            { "ID_": "r1", "Name": "Gun 2", "Tags": ["c"] },
            { "Name": "New", "Runtime": "12345", "Image": "images/New.jpg", "ID_": "r2" },
        ])
    );
    let image = RuntimeResourceID::from_raw_string(
        "[assembly:/_pro/online/default/cloudstorage/resources/images/new.jpg].pc_gfx",
    );
    assert_eq!(
        references(&repository),
        vec![(IN_CHUNK1, 0x1F), (12345, 0x9F), (u64::from(image), 0x9F)]
    );
}

#[test]
fn contracts_are_added_and_listed_in_destinations() {
    let env = Env::new();
    let m = content_mod(
        &env,
        "Contracts",
        &[
            (
                "mine.contract.json",
                r#"{ "Metadata": { "Id": "my-contract" },
                     "SMF": { "destinations": { "addToDestinations": true, "placeAfter": "m1" } } }"#,
            ),
            (
                "game.contract.json",
                r#"{ "Metadata": { "Id": "game-contract", "Changed": true } }"#,
            ),
        ],
    );
    let desired = env.build(&[m]).unwrap();
    let archive = placed(&env, &desired, "Runtime/chunk0patch300.rpkg");

    let hash = u64::from(json::contract_hash("my-contract"));
    let contracts = resource(&archive, json::CONTRACTS);
    assert_eq!(
        ores_hashes(&contracts.data),
        vec![
            (GAME_CONTRACT, "game-contract".to_owned()),
            (hash, "my-contract".to_owned())
        ]
    );
    assert_eq!(
        references(&contracts),
        vec![(GAME_CONTRACT, 0x1F), (hash, 0x9F)]
    );

    let mine = resource(&archive, hash);
    assert_eq!(mine.resource_type, "JSON");
    assert_eq!(json_of(&mine.data)["Metadata"]["Id"], "my-contract");
    // replacing one of the game's keeps its header
    let game = resource(&archive, GAME_CONTRACT);
    assert_eq!(json_of(&game.data)["Metadata"]["Changed"], true);
    assert_eq!(references(&game), vec![(IN_CHUNK1, 0x1F)]);

    let menu = json_of(&resource(&archive, json::DESTINATIONS).data);
    let ids: Vec<_> = menu["Root"]["Children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["Id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["m1", "my-contract", "m2"]);
    assert_eq!(menu["Root"]["Children"][1]["NarrativeContext"], "Mission");
}

#[test]
fn json_patches_apply_to_each_kind_of_resource() {
    let env = Env::new();
    let patch = |file: u64, kind: &str, ops: &str| {
        format!(r#"{{ "file": "{file:016X}", "type": "{kind}", "patch": {ops} }}"#)
    };
    let files = [
        (
            "menu.JSON.patch.json",
            patch(
                json::DESTINATIONS,
                "JSON",
                r#"[{ "op": "add", "path": "/Root/Children/-", "value": { "Id": "m3" } }]"#,
            ),
        ),
        (
            "repo.JSON.patch.json",
            patch(
                json::REPOSITORY,
                "REPO",
                r#"[{ "op": "replace", "path": "/r1/Name", "value": "Patched" }]"#,
            ),
        ),
        (
            "unlockables.JSON.patch.json",
            patch(
                json::UNLOCKABLES,
                "ORES",
                r#"[{ "op": "remove", "path": "/GUN" }]"#,
            ),
        ),
        (
            "contracts.JSON.patch.json",
            patch(
                json::CONTRACTS,
                "ORES",
                r#"[{ "op": "add", "path": "/00CC000000000001", "value": "added" }]"#,
            ),
        ),
    ];
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(f, c)| (*f, c.as_str())).collect();
    let m = content_mod(&env, "Patches", &borrowed);
    let desired = env.build(&[m]).unwrap();
    let archive = placed(&env, &desired, "Runtime/chunk0patch300.rpkg");

    let menu = json_of(&resource(&archive, json::DESTINATIONS).data);
    assert_eq!(menu["Root"]["Children"][2]["Id"], "m3");
    let repository = json_of(&resource(&archive, json::REPOSITORY).data);
    assert_eq!(repository[0]["Name"], "Patched");
    assert_eq!(
        ores_json(&resource(&archive, json::UNLOCKABLES).data),
        serde_json::json!([])
    );
    assert_eq!(
        ores_hashes(&resource(&archive, json::CONTRACTS).data),
        vec![
            (GAME_CONTRACT, "game-contract".to_owned()),
            (0x00CC_0000_0000_0001, "added".to_owned())
        ]
    );

    let missing = content_mod(
        &env,
        "Missing",
        &[(
            "x.JSON.patch.json",
            r#"{ "file": "00DD000000000001", "patch": [] }"#,
        )],
    );
    let err = env.build(&[missing]).unwrap_err().to_string();
    assert!(err.contains("00DD000000000001"), "{err}");
}

#[test]
fn blobs_are_added_to_the_online_resources() {
    let env = Env::new();
    let m = env.installed(
        "blobs",
        &[
            (
                "manifest.json",
                br#"{ id: "B", name: "B", blobsFolders: ["blobs"] }"#,
            ),
            ("blobs/images/New.JPG", b"jpeg"),
            ("blobs/data/thing.json", b"{}"),
        ],
    );
    let desired = env.build(&[m]).unwrap();
    let archive = placed(&env, &desired, "Runtime/chunk0patch300.rpkg");

    let (image, image_type) = json::blob("images/new.jpg");
    let (data, data_type) = json::blob("data/thing.json");
    assert_eq!((image_type.as_str(), data_type.as_str()), ("GFXI", "JSON"));
    let blob = resource(&archive, u64::from(image));
    assert_eq!(
        (blob.resource_type.as_str(), blob.data.as_slice()),
        ("GFXI", &b"jpeg"[..])
    );

    let ores = resource(&archive, json::BLOBS);
    let entries = ores_hashes(&ores.data);
    assert_eq!(entries.len(), 3);
    assert!(entries.contains(&(u64::from(image), "images/new.jpg".to_owned())));
    assert!(entries.contains(&(u64::from(data), "data/thing.json".to_owned())));
    let refs = references(&ores);
    assert!(refs.contains(&(u64::from(image), 0x9F)) && refs.contains(&(u64::from(data), 0x9F)));
}

#[test]
fn ores_round_trip() {
    for ores in [
        ores::Ores::Json(UNLOCKABLES_JSON.to_owned()),
        ores::Ores::Json(String::new()),
        ores::Ores::Hashes(vec![
            (1, "a".to_owned()),
            (2, "bcdef".to_owned()),
            (3, "gh".to_owned()),
        ]),
        ores::Ores::Hashes(Vec::new()),
    ] {
        assert_eq!(ores::Ores::parse(&ores.serialize()).unwrap(), ores);
    }
}

#[test]
fn deep_merge_replaces_arrays_and_merges_objects() {
    let mut target = serde_json::json!({ "a": { "b": 1, "c": [1, 2] }, "d": 1 });
    json::deep_merge(
        &mut target,
        serde_json::json!({ "a": { "c": [3], "e": null }, "d": { "f": 2 } }),
    );
    assert_eq!(
        target,
        serde_json::json!({ "a": { "b": 1, "c": [3], "e": null }, "d": { "f": 2 } })
    );
}

const ENTITY_TEMP: u64 = 0x00C0_0000_0000_0001;
const ENTITY_TBLU: u64 = 0x00C0_0000_0000_0002;
const CHILD: &str = "abcdef0000000002";

/// A small entity: a root with a child that has one property.
fn entity_json() -> Value {
    serde_json::json!({
        "tempHash": "00C0000000000001",
        "tbluHash": "00C0000000000002",
        "rootEntity": "abcdef0000000001",
        "entities": {
            "abcdef0000000001": {
                "parent": null,
                "name": "Root",
                "factory": "00D0000000000001",
                "blueprint": "00D0000000000002",
            },
            CHILD: {
                "parent": "abcdef0000000001",
                "name": "Child",
                "factory": "00D0000000000003",
                "blueprint": "00D0000000000004",
                "properties": { "m_nCount": { "type": "int32", "value": 1 } },
            },
        },
        "propertyOverrides": [],
        "overrideDeletes": [],
        "pinConnectionOverrides": [],
        "pinConnectionOverrideDeletes": [],
        "externalScenes": [],
        "subType": "template",
        "quickEntityVersion": 3.1,
        "extraFactoryDependencies": [],
        "extraBlueprintDependencies": [],
        "comments": [],
    })
}

fn entity_patch(operations: Value) -> String {
    serde_json::json!({
        "tempHash": "00C0000000000001",
        "tbluHash": "00C0000000000002",
        "patch": operations,
        "patchVersion": 6,
    })
    .to_string()
}

/// The entity in a built archive, read back.
fn built_entity(archive: &Path) -> Value {
    let factory = resource(archive, ENTITY_TEMP);
    let blueprint = resource(archive, ENTITY_TBLU);
    assert_eq!(factory.resource_type, "TEMP");
    assert_eq!(blueprint.resource_type, "TBLU");
    let entity = entity::decode((ENTITY_TEMP, &factory), (ENTITY_TBLU, &blueprint)).unwrap();
    serde_json::to_value(entity).unwrap()
}

#[test]
fn entities_are_built_from_quickentity_json() {
    let env = Env::new();
    let m = content_mod(
        &env,
        "Entity",
        &[("thing.entity.json", &entity_json().to_string())],
    );
    let desired = env.build(&[m]).unwrap();
    let archive = placed(&env, &desired, "Runtime/chunk0patch300.rpkg");

    let factory = resource(&archive, ENTITY_TEMP);
    let referenced: Vec<u64> = references(&factory).iter().map(|(r, _)| *r).collect();
    assert!(
        referenced.contains(&0x00D0_0000_0000_0001),
        "{referenced:X?}"
    );
    assert!(
        referenced.contains(&0x00D0_0000_0000_0003),
        "{referenced:X?}"
    );

    let entity = built_entity(&archive);
    assert_eq!(entity["entities"], entity_json()["entities"]);
    assert_eq!(entity["rootEntity"], "abcdef0000000001");
}

#[test]
fn entity_patches_apply_in_order_to_the_current_entity() {
    let env = Env::new();
    let original =
        entity::generate(&entity::parse_entity(entity_json().to_string().as_bytes()).unwrap())
            .unwrap();
    let in_game = |(hash, loaded): &(u64, resource::Loaded)| {
        let references: Vec<u64> = references(loaded).iter().map(|(r, _)| *r).collect();
        (
            *hash,
            loaded.resource_type.clone(),
            loaded.data.clone(),
            references,
        )
    };
    let (factory, blueprint) = (in_game(&original.factory), in_game(&original.blueprint));
    env.archive(
        "chunk2",
        PatchId::Patch(2),
        &[
            (factory.0, &factory.1, factory.2, &factory.3),
            (blueprint.0, &blueprint.1, blueprint.2, &blueprint.3),
        ],
    );

    let set_count = |n: i64| {
        entity_patch(serde_json::json!([
            { "SubEntityOperation": [CHILD, { "SetPropertyValue": { "property_name": "m_nCount", "value": n } }] }
        ]))
    };
    let add_flag = entity_patch(serde_json::json!([
        { "SubEntityOperation": [CHILD, { "AddProperty": ["m_bFlag", { "type": "bool", "value": true }] }] },
        { "SubEntityOperation": [CHILD, { "SetName": "Renamed" }] }
    ]));
    let manifest = r#"{ id: "Patches", name: "Patches", contentFolders: ["content"] }"#;
    let first = env.installed(
        "first",
        &[
            ("manifest.json", manifest.as_bytes()),
            (
                "content/chunk2/a.entity.patch.json",
                set_count(2).as_bytes(),
            ),
            ("content/chunk2/b.entity.patch.json", add_flag.as_bytes()),
        ],
    );
    let later = env.installed(
        "later",
        &[
            (
                "manifest.json",
                br#"{ id: "Later", name: "Later", contentFolders: ["content"] }"#,
            ),
            (
                "content/chunk2/later.entity.patch.json",
                set_count(3).as_bytes(),
            ),
        ],
    );
    let desired = env.build(&[first, later]).unwrap();

    let entity = built_entity(&placed(&env, &desired, "Runtime/chunk2patch300.rpkg"));
    let child = &entity["entities"][CHILD];
    // The later mod's patch applies on top of the first's.
    assert_eq!(child["properties"]["m_nCount"]["value"], 3);
    assert_eq!(child["properties"]["m_bFlag"]["value"], true);
    assert_eq!(child["name"], "Renamed");
}

#[test]
fn entities_that_cant_be_deployed_fail() {
    let env = Env::new();
    let mut old = entity_json();
    old["quickEntityVersion"] = serde_json::json!(2.2);
    let mut old_patch: Value = serde_json::from_str(&entity_patch(serde_json::json!([]))).unwrap();
    old_patch["patchVersion"] = serde_json::json!(5);
    let mut elsewhere: Value = serde_json::from_str(&entity_patch(serde_json::json!([]))).unwrap();
    elsewhere["tempHash"] = serde_json::json!("00C0000000000009");
    let m = content_mod(
        &env,
        "Old",
        &[
            ("old.entity.json", &old.to_string()),
            ("old.entity.patch.json", &old_patch.to_string()),
            ("missing.entity.patch.json", &elsewhere.to_string()),
        ],
    );
    let err = env.build(&[m]).unwrap_err().to_string();
    assert!(
        err.contains("old.entity.json: it's for QuickEntity 2.2"),
        "{err}"
    );
    assert!(
        err.contains("old.entity.patch.json: it's patch version 5"),
        "{err}"
    );
    assert!(
        err.contains("missing.entity.patch.json: the game has no TEMP 00C0000000000009 to patch"),
        "{err}"
    );
}

const TEXTURE_TEXT: u64 = 0x00E0_0000_0000_0001;
const TEXTURE_TEXD: u64 = 0x00E0_0000_0000_0002;

/// An uncompressed TGA, top row first: greyscale with one byte per pixel,
/// otherwise BGRA.
fn tga(width: u16, height: u16, pixels: &[u8]) -> Vec<u8> {
    let grey = pixels.len() == usize::from(width) * usize::from(height);
    let mut tga = vec![0, 0, if grey { 3 } else { 2 }, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    tga.extend_from_slice(&width.to_le_bytes());
    tga.extend_from_slice(&height.to_le_bytes());
    tga.push(if grey { 8 } else { 32 });
    tga.push(if grey { 0x20 } else { 0x28 });
    tga.extend_from_slice(pixels);
    tga
}

/// HMTextureTools' meta for a colour texture in `format`, LZ4 compressed.
fn texture_meta(format: u16) -> Vec<u8> {
    let mut meta = vec![0; 16];
    meta[4] = 0x40;
    meta[8..10].copy_from_slice(&format.to_le_bytes());
    meta[10] = 1;
    meta
}

fn texture_mod(env: &Env, name: &str, tga: &[u8], meta: Option<&[u8]>) -> InstalledMod {
    let manifest = format!(r#"{{ id: "{name}", name: "{name}", contentFolders: ["content"] }}"#);
    let tga_path = format!("content/chunk0/{name}.texture.tga");
    let meta_path = format!("{tga_path}.meta");
    let mut files: Vec<(&str, &[u8])> =
        vec![("manifest.json", manifest.as_bytes()), (&tga_path, tga)];
    if let Some(meta) = meta {
        files.push((&meta_path, meta));
    }
    env.installed(name, &files)
}

#[test]
fn textures_are_converted_to_a_text() {
    let env = Env::new();
    let pixels: Vec<u8> = (0..=255).collect();
    let m = texture_mod(
        &env,
        &format!("{TEXTURE_TEXT:016X}"),
        &tga(16, 16, &pixels),
        Some(&texture_meta(0x42)),
    );
    let desired = env.build(&[m]).unwrap();
    let archive = placed(&env, &desired, "Runtime/chunk0patch300.rpkg");

    let text = resource(&archive, TEXTURE_TEXT);
    assert_eq!(text.resource_type, "TEXT");
    assert!(text.header.scrambled);
    assert!(text.header.references.is_empty());
    assert_eq!(text.header.video_memory, 0);
    let texture =
        glacier_texture::texture_map::TextureMap::from_memory(&text.data, GlacierGame::HM3)
            .unwrap();
    assert_eq!((texture.width(), texture.height()), (16, 16));
    assert_eq!(texture.num_mip_levels(), 1);
    assert!(texture.flags().lz4_compression());
    assert_eq!(texture.mipmap(0).unwrap().data, pixels);
}

#[test]
fn textures_named_with_a_texd_are_split_between_text_and_texd() {
    let env = Env::new();
    let pixels: Vec<u8> = (0..256 * 256)
        .flat_map(|i| [i as u8, (i >> 8) as u8, 0, 255])
        .collect();
    let m = texture_mod(
        &env,
        &format!("{TEXTURE_TEXT:016X}~{TEXTURE_TEXD:016X}"),
        &tga(256, 256, &pixels),
        Some(&texture_meta(0x49)),
    );
    let desired = env.build(&[m]).unwrap();
    let archive = placed(&env, &desired, "Runtime/chunk0patch300.rpkg");

    let text = resource(&archive, TEXTURE_TEXT);
    assert_eq!(references(&text), [(TEXTURE_TEXD, json::LOOKUP_REFERENCE)]);
    let texd = resource(&archive, TEXTURE_TEXD);
    assert_eq!(texd.resource_type, "TEXD");
    assert!(!texd.header.scrambled);
    // The top mip, which only the TEXD has.
    assert_eq!(text.header.video_memory, 256 * 256 / 2);
    assert_eq!(texd.header.video_memory, text.header.video_memory);

    let mut texture =
        glacier_texture::texture_map::TextureMap::from_memory(&text.data, GlacierGame::HM3)
            .unwrap();
    // The TEXT on its own holds all but the top mip.
    assert_eq!((texture.width(), texture.height()), (128, 128));
    texture.set_mipblock1(
        glacier_texture::mipblock::MipblockData::from_memory(&texd.data, GlacierGame::HM3).unwrap(),
    );
    assert_eq!((texture.width(), texture.height()), (256, 256));
    assert_eq!(texture.num_mip_levels(), 9);
    let top = texture.mipmap(0).unwrap();
    assert_eq!((top.width, top.height), (256, 256));
    // BC1: 8 bytes per 4x4 block.
    assert_eq!(top.data.len(), 64 * 64 * 8);
}

#[test]
fn textures_that_cant_be_converted_fail() {
    let env = Env::new();
    let image = tga(16, 16, &[0; 256]);
    let no_meta = texture_mod(&env, &format!("{TEXTURE_TEXT:016X}"), &image, None);
    let bad_name = texture_mod(&env, "not-a-hash", &image, Some(&texture_meta(0x42)));
    let bad_size = texture_mod(
        &env,
        &format!("{TEXTURE_TEXD:016X}"),
        &tga(12, 16, &[0; 192]),
        Some(&texture_meta(0x42)),
    );
    let err = env
        .build(&[no_meta, bad_name, bad_size])
        .unwrap_err()
        .to_string();
    assert!(err.contains("no .meta"), "{err}");
    assert!(err.contains("have to be named"), "{err}");
    assert!(err.contains("power of two"), "{err}");
}
