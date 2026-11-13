//! Deploys FTL: Faster Than Light mods.
//!
//! Since 1.6, the game reads everything from one archive, `ftl.dat`, so
//! deploying builds a new one: the game's own, with mods' files patched in
//! as Slipstream (the community's original mod manager) does it:
//!
//! - A mod is a zip (often renamed `.ftl`) with `data/`, `audio/`,
//!   `fonts/` and `img/` folders, whose files replace the game's at the
//!   same paths. `mod-appendix/` has the mod's description, and anything
//!   else is left out.
//! - `x.xml.append` (or `x.append.xml`) is merged into `x.xml`; see
//!   [`patch`].
//! - `x.xml.rawappend` (or `x.rawappend.xml`) is added to the end of
//!   `x.xml` as text, and `x.xml.rawclobber` (or `x.rawclobber.xml`)
//!   replaces it.
//! - `.txt` files are converted to UTF-8 with Windows line endings, which
//!   some of the game's (`layout.txt`) need.
//!
//! Mods are applied in load order, each to the result of the ones before.
//! The game finds files in the archive case-insensitively, so mods' paths
//! are matched that way too.
//!
//! A mod whose files are in a folder of their own is deployed from inside
//! it, and an `.ftl` at the top of a mod (as in Hyperspace's release zip)
//! is opened and deployed as part of the mod.
//!
//! ```toml
//! [deployer]
//! kind = "ftl"
//! ```
//!
//! If ftlman has patched the game, it keeps the game's own archive as
//! `ftl.dat.vanilla`, and that's what mods are applied to.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use patchwork_core::deploy::{
    BuildContext, DeployContext, Deployer, DesiredState, Dest, GameFiles, InstalledMod, Placement,
    Src,
};
use patchwork_core::error::IoContext;
use patchwork_core::event::Event;
use patchwork_core::ext::{Extension, Registrar};
use patchwork_core::hash::ContentHash;
use patchwork_core::relpath::RelPath;
use patchwork_core::{Error, Result};

pub mod patch;
pub mod pkg;
pub mod xml;

use patch::Doc;
use pkg::Data;

/// The `ftl` extension: the deployer for FTL: Faster Than Light.
pub struct Ftl;

impl Extension for Ftl {
    fn id(&self) -> &'static str {
        "ftl"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(FtlDeployer);
    }
}

pub struct FtlDeployer;

/// Where the game keeps its archive: Linux, Windows and macOS.
const ARCHIVES: [&str; 3] = [
    "data/ftl.dat",
    "ftl.dat",
    "FTL.app/Contents/Resources/ftl.dat",
];

/// The folders of a mod that go into the archive.
const INSTALLED: [&str; 4] = ["data", "audio", "fonts", "img"];

/// The folder of a mod with its description, which isn't installed.
const APPENDIX: &str = "mod-appendix";

/// Shown as the mod the archive comes from.
const BUILT_BY: &str = "FTL mods";

/// The built archive, in the build's output.
const BUILT: &str = "ftl.dat";

/// The game's archive: where it goes, and the unpatched copy to start
/// from.
fn locate(game: &GameFiles<'_>) -> Option<(RelPath, PathBuf)> {
    ARCHIVES.iter().find_map(|rel| {
        let rel = RelPath::new(rel).unwrap();
        let original = game.original(&rel)?;
        let vanilla = RelPath::new(&format!("{rel}.vanilla")).unwrap();
        Some((rel, game.original(&vanilla).unwrap_or(original)))
    })
}

impl FtlDeployer {
    fn error(&self, message: String) -> Error {
        Error::Deployer {
            kind: self.kind().to_owned(),
            message,
        }
    }
}

impl Deployer for FtlDeployer {
    fn kind(&self) -> &str {
        "ftl"
    }

    /// Everything's in [`Deployer::build`], since every mod ends up in the
    /// one archive.
    fn plan(&self, _: &DeployContext<'_>, _: &[InstalledMod]) -> Result<DesiredState> {
        Ok(DesiredState::default())
    }

    fn builds(&self, _: &DeployContext<'_>) -> bool {
        true
    }

    fn build(
        &self,
        _: &DeployContext<'_>,
        build: &BuildContext<'_>,
        mods: &[InstalledMod],
    ) -> Result<DesiredState> {
        let mut desired = DesiredState::default();
        if mods.is_empty() {
            return Ok(desired);
        }
        let (rel, base) = locate(&build.game).ok_or_else(|| {
            self.error(
                "the game's ftl.dat wasn't found; only FTL 1.6 and later are supported".to_owned(),
            )
        })?;
        build.reporter.report(Event::Building {
            step: "reading ftl.dat".to_owned(),
        });
        let index = pkg::read_index(&mut BufReader::new(File::open(&base).at(&base)?))
            .map_err(|e| Error::parse(&base, e))?;
        let mut files = Files::new(&base, index)?;
        for m in mods {
            build.reporter.report(Event::Building {
                step: m.name.clone(),
            });
            let entries = mod_entries(m, build.cache)?;
            let before = desired.warnings.len();
            files
                .apply(&entries, &mut desired.warnings)
                .map_err(|e| self.error(format!("{}: {e}", m.name)))?;
            for w in &mut desired.warnings[before..] {
                *w = format!("{}: {w}", m.name);
            }
            if !entries.iter().any(|(rel, _)| installed(rel)) {
                desired.warnings.push(format!(
                    "{} has no data/, audio/, fonts/ or img/ folder, so nothing from it is deployed",
                    m.name
                ));
            }
        }
        build.reporter.report(Event::Building {
            step: "writing ftl.dat".to_owned(),
        });
        let out = build.out.join(BUILT);
        pkg::write(&out, Some(&base), &files.finish()).at(&out)?;
        desired.placements.push(Placement {
            dst: Dest::game(rel),
            src: Src::Built(RelPath::new(BUILT).unwrap()),
            mod_name: BUILT_BY.to_owned(),
        });
        Ok(desired)
    }

    /// The size and modification time of the archive mods are applied to.
    fn fingerprint(
        &self,
        _: &DeployContext<'_>,
        game: &GameFiles<'_>,
    ) -> Result<Option<ContentHash>> {
        let Some((rel, base)) = locate(game) else {
            return Ok(None);
        };
        let meta = fs::metadata(&base).at(&base)?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let key = format!("{rel} {} {} {modified}", base.display(), meta.len());
        Ok(Some(ContentHash::of(key.as_bytes())))
    }
}

/// Whether a mod's file (by its path in the mod) goes into the archive.
fn installed(rel: &str) -> bool {
    rel.split_once('/')
        .is_some_and(|(top, _)| INSTALLED.iter().any(|i| top.eq_ignore_ascii_case(i)))
}

/// A mod's files, as their paths from the mod's top and where they are.
/// `.ftl` files at the mod's top are unpacked (into `cache`) and their
/// files included, and if the rest are all in one folder without any mod
/// folders, they're taken from inside it.
fn mod_entries(m: &InstalledMod, cache: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut nested = Vec::new();
    let mut plain = Vec::new();
    for file in &m.files {
        let rel = file.rel.as_str();
        if !rel.contains('/') && rel.to_ascii_lowercase().ends_with(".ftl") {
            let dir = cache.join("unpacked").join(m.hash.to_string()).join(rel);
            unpack(&file.src, &dir)?;
            for entry in walk(&dir)? {
                nested.push(entry);
            }
        } else {
            plain.push((rel.to_owned(), file.src.clone()));
        }
    }
    let is_mod_folder = |rel: &str| {
        installed(rel)
            || rel
                .split_once('/')
                .is_some_and(|(top, _)| top.eq_ignore_ascii_case(APPENDIX))
    };
    if !plain.iter().any(|(rel, _)| is_mod_folder(rel)) {
        let top = |rel: &str| rel.split_once('/').map(|(t, _)| t.to_owned());
        if let Some(Some(first)) = plain.first().map(|(rel, _)| top(rel))
            && plain
                .iter()
                .all(|(rel, _)| top(rel).as_deref() == Some(&first))
        {
            for (rel, _) in &mut plain {
                *rel = rel[first.len() + 1..].to_owned();
            }
        }
    }
    nested.extend(plain);
    Ok(nested)
}

/// Unpacks the zip at `archive` into `dir`, unless that's been done.
fn unpack(archive: &Path, dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let tmp = dir.with_extension("partial");
    let _ = fs::remove_dir_all(&tmp);
    let bad = |e: zip::result::ZipError| Error::parse(archive, e);
    let mut zip = zip::ZipArchive::new(File::open(archive).at(archive)?).map_err(bad)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(bad)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let path = tmp.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).at(parent)?;
        }
        let mut out = File::create(&path).at(&path)?;
        std::io::copy(&mut entry, &mut out).at(&path)?;
        out.flush().at(&path)?;
    }
    fs::create_dir_all(&tmp).at(&tmp)?;
    fs::rename(&tmp, dir).at(dir)?;
    Ok(())
}

/// Every file under `dir`, by its path from there with `/`s.
fn walk(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_owned()];
    while let Some(d) = dirs.pop() {
        for entry in fs::read_dir(&d).at(&d)? {
            let path = entry.at(&d)?.path();
            if path.is_dir() {
                dirs.push(path);
            } else if let Ok(rel) = path.strip_prefix(dir) {
                let rel: Vec<_> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect();
                files.push((rel.join("/"), path));
            }
        }
    }
    files.sort();
    Ok(files)
}

/// What a file in the archive will have.
enum Content {
    Data(Data),
    /// A file being patched.
    Doc(Doc),
}

/// What a mod's file does.
enum Change<'a> {
    Replace(String, Data),
    Append(String, &'a Path),
    RawAppend(String, &'a Path),
}

/// The suffixes a mod's file can have, after the `.xml` its target has.
const APPEND: [&str; 2] = [".xml.append", ".append.xml"];
const RAW_APPEND: [&str; 2] = [".xml.rawappend", ".rawappend.xml"];
const RAW_CLOBBER: [&str; 2] = [".xml.rawclobber", ".rawclobber.xml"];

/// `rel` without one of `suffixes` (ignoring case), with `.xml` instead.
fn target(rel: &str, suffixes: &[&str]) -> Option<String> {
    let lower = rel.to_ascii_lowercase();
    suffixes
        .iter()
        .find(|s| lower.ends_with(*s))
        .map(|s| format!("{}.xml", &rel[..rel.len() - s.len()]))
}

fn change<'a>(rel: &str, src: &'a Path) -> Result<Option<Change<'a>>> {
    if !installed(rel)
        || rel
            .rsplit('/')
            .next()
            .is_some_and(|n| n.eq_ignore_ascii_case(".DS_Store"))
    {
        return Ok(None);
    }
    Ok(Some(if let Some(t) = target(rel, &APPEND) {
        Change::Append(t, src)
    } else if let Some(t) = target(rel, &RAW_APPEND) {
        Change::RawAppend(t, src)
    } else if let Some(t) = target(rel, &RAW_CLOBBER) {
        Change::Replace(t, Data::File(src.to_owned()))
    } else if rel.to_ascii_lowercase().ends_with(".txt") {
        let text = xml::decode(&fs::read(src).at(src)?);
        Change::Replace(rel.to_owned(), Data::Bytes(crlf(text.as_bytes())))
    } else {
        Change::Replace(rel.to_owned(), Data::File(src.to_owned()))
    }))
}

/// `bytes` with every line ending as `\r\n`.
fn crlf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 32);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                out.extend_from_slice(b"\r\n");
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            b'\n' => out.extend_from_slice(b"\r\n"),
            b => out.push(b),
        }
        i += 1;
    }
    out
}

/// The archive being built: each file by its lowercased path, with its
/// path as spelled.
struct Files {
    base: File,
    files: BTreeMap<String, (String, Content)>,
}

impl Files {
    fn new(base: &Path, index: Vec<pkg::Entry>) -> Result<Self> {
        let files = index
            .into_iter()
            .map(|e| {
                let data = Data::Base {
                    offset: e.offset,
                    size: e.size,
                };
                (e.path.to_ascii_lowercase(), (e.path, Content::Data(data)))
            })
            .collect();
        Ok(Self {
            base: File::open(base).at(base)?,
            files,
        })
    }

    fn set(&mut self, path: String, content: Content) {
        let key = path.to_ascii_lowercase();
        match self.files.get_mut(&key) {
            Some(existing) => existing.1 = content,
            None => {
                self.files.insert(key, (path, content));
            }
        }
    }

    /// The text of the file at `path`, if there is one.
    fn text(&mut self, path: &str) -> std::result::Result<Option<String>, String> {
        let Some((_, content)) = self.files.get(&path.to_ascii_lowercase()) else {
            return Ok(None);
        };
        let bytes = match content {
            Content::Doc(doc) => return Ok(Some(doc.write())),
            Content::Data(Data::Base { offset, size }) => {
                pkg::read_entry(&mut self.base, *offset, *size).map_err(|e| e.to_string())?
            }
            Content::Data(Data::File(p)) => {
                fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?
            }
            Content::Data(Data::Bytes(b)) => b.clone(),
        };
        Ok(Some(xml::decode(&bytes)))
    }

    /// Applies one mod's files: first the ones that replace files, then the
    /// ones that patch them.
    fn apply(
        &mut self,
        entries: &[(String, PathBuf)],
        warnings: &mut Vec<String>,
    ) -> std::result::Result<(), String> {
        let mut changes = Vec::new();
        for (rel, src) in entries {
            if let Some(change) = change(rel, src).map_err(|e| e.to_string())? {
                changes.push((rel, change));
            }
        }
        changes.sort_by_key(|(_, c)| !matches!(c, Change::Replace(..)));
        for (rel, change) in changes {
            let read = |src: &Path| {
                fs::read(src)
                    .map(|b| xml::decode(&b))
                    .map_err(|e| format!("{rel}: {e}"))
            };
            match change {
                Change::Replace(path, data) => self.set(path, Content::Data(data)),
                Change::Append(path, src) => {
                    let patch = read(src)?;
                    let key = path.to_ascii_lowercase();
                    let mut doc = match self.files.get_mut(&key) {
                        Some((_, content @ Content::Doc(_))) => {
                            let empty = Content::Data(Data::Bytes(Vec::new()));
                            let Content::Doc(doc) = std::mem::replace(content, empty) else {
                                unreachable!()
                            };
                            doc
                        }
                        _ => Doc::parse(&self.text(&path)?.unwrap_or_default()),
                    };
                    let mut found = Vec::new();
                    doc.append(&patch, &mut found)
                        .map_err(|e| format!("{rel}: {e}"))?;
                    warnings.extend(found.into_iter().map(|w| format!("{rel}: {w}")));
                    self.set(path, Content::Doc(doc));
                }
                Change::RawAppend(path, src) => {
                    let patch = read(src)?;
                    let base = self.text(&path)?.unwrap_or_default();
                    let text = patch::raw_append(&base, &patch);
                    self.set(path, Content::Data(Data::Bytes(text.into_bytes())));
                }
            }
        }
        Ok(())
    }

    /// Every file, ready to [`pkg::write`].
    fn finish(self) -> Vec<(String, Data)> {
        self.files
            .into_values()
            .map(|(path, content)| match content {
                Content::Data(data) => (path, data),
                Content::Doc(doc) => (path, Data::Bytes(doc.write().into_bytes())),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
