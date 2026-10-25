//! The folders a [`DeployTarget`](super::DeployTarget) deploys into. Most
//! games only have their own folder (the `game` root), but some also read
//! files from elsewhere, like the load order Skyrim keeps in AppData.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::game::{FilePattern, Folder, RootDef, RootDefs, RootPath};
use crate::relpath::RelPath;
use crate::{Error, Result};

/// The name of one of a target's roots: lowercase letters, digits and `-`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RootId(String);

impl RootId {
    /// The game's own folder, which every target has.
    pub const GAME: &str = "game";

    pub fn new(id: &str) -> std::result::Result<Self, String> {
        let valid = !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if valid {
            Ok(Self(id.to_owned()))
        } else {
            Err(format!(
                "`{id}` isn't a valid root; use lowercase letters, digits and `-`"
            ))
        }
    }

    pub fn game() -> Self {
        Self(Self::GAME.to_owned())
    }

    pub fn is_game(&self) -> bool {
        self.0 == Self::GAME
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RootId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for RootId {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, String> {
        Self::new(&s)
    }
}

impl From<RootId> for String {
    fn from(id: RootId) -> String {
        id.0
    }
}

/// Where a file goes: a path inside one of the target's roots. Written
/// `appdata:plugins.txt`, or just the path for the `game` root, which is
/// unambiguous because neither root ids nor [`RelPath`]s can contain `:`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Dest {
    pub root: RootId,
    pub path: RelPath,
}

impl Dest {
    pub fn new(root: RootId, path: RelPath) -> Self {
        Self { root, path }
    }

    /// `path` in the game's own folder.
    pub fn game(path: RelPath) -> Self {
        Self::new(RootId::game(), path)
    }

    /// The folder this is in, if it's not directly in its root.
    pub fn parent(&self) -> Option<Dest> {
        Some(Self::new(self.root.clone(), self.path.parent()?))
    }
}

impl fmt::Display for Dest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.root.is_game() {
            write!(f, "{}", self.path)
        } else {
            write!(f, "{}:{}", self.root, self.path)
        }
    }
}

impl fmt::Debug for Dest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl FromStr for Dest {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.split_once(':') {
            Some((root, path)) => Ok(Self::new(RootId::new(root)?, RelPath::new(path)?)),
            None => Ok(Self::game(RelPath::new(s)?)),
        }
    }
}

impl TryFrom<String> for Dest {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, String> {
        s.parse()
    }
}

impl From<Dest> for String {
    fn from(dest: Dest) -> String {
        dest.to_string()
    }
}

/// Where each of a target's roots is on disk. There's always a `game` root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots(BTreeMap<RootId, PathBuf>);

impl Roots {
    /// Just the game's own folder.
    pub fn game(path: &Path) -> Self {
        Self(BTreeMap::from([(RootId::game(), path.to_owned())]))
    }

    /// Adds another root, or moves one.
    pub fn with(mut self, root: RootId, path: &Path) -> Self {
        self.0.insert(root, path.to_owned());
        self
    }

    pub fn game_path(&self) -> &Path {
        &self.0[&RootId::game()]
    }

    pub fn get(&self, root: &RootId) -> Option<&Path> {
        self.0.get(root).map(PathBuf::as_path)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&RootId, &Path)> {
        self.0.iter().map(|(id, path)| (id, path.as_path()))
    }

    /// Where `dest` is on disk. An error for a root the target doesn't
    /// have, which is how a deployer naming one gets caught.
    pub fn path(&self, dest: &Dest) -> Result<PathBuf> {
        let root = self.get(&dest.root).ok_or_else(|| {
            Error::invalid(format!(
                "{dest} is in a folder called `{}`, which this game doesn't have",
                dest.root
            ))
        })?;
        Ok(dest.path.under(root))
    }
}

/// Where the [`Folder`]s a root's path starts from are, either on this
/// machine or inside a Wine prefix. `None` for those there aren't.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folders {
    pub home: Option<PathBuf>,
    pub documents: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    pub data: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

impl Folders {
    /// This machine's own.
    pub fn host() -> Self {
        let base = directories::BaseDirs::new();
        let user = directories::UserDirs::new();
        let base_dir =
            |f: fn(&directories::BaseDirs) -> &Path| base.as_ref().map(|b| f(b).to_owned());
        Self {
            home: base_dir(directories::BaseDirs::home_dir),
            documents: user
                .as_ref()
                .and_then(|u| u.document_dir())
                .map(Path::to_owned),
            // on Windows these are Roaming and Local AppData
            appdata: base_dir(directories::BaseDirs::config_dir).filter(|_| cfg!(windows)),
            local_appdata: base_dir(directories::BaseDirs::data_local_dir)
                .filter(|_| cfg!(windows)),
            data: base_dir(directories::BaseDirs::data_dir),
            config: base_dir(directories::BaseDirs::config_dir),
        }
    }

    /// What a Windows game running in the Wine prefix `prefix` (the folder
    /// with `drive_c` in it) sees as them, if it looks like one. Proton's
    /// prefixes have a `steamuser`; others are named after the user.
    pub fn wine(prefix: &Path) -> Option<Self> {
        let users = prefix.join("drive_c/users");
        let named = ["steamuser".to_owned()]
            .into_iter()
            .chain(std::env::var("USER"))
            .map(|name| users.join(name))
            .find(|dir| dir.is_dir());
        let home = named.or_else(|| {
            let mut dirs = std::fs::read_dir(&users)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && !p.ends_with("Public"));
            let only = dirs.next()?;
            dirs.next().is_none().then_some(only)
        })?;
        let roaming = home.join("AppData/Roaming");
        Some(Self {
            documents: Some(home.join("Documents")),
            appdata: Some(roaming.clone()),
            local_appdata: Some(home.join("AppData/Local")),
            data: Some(roaming.clone()),
            config: Some(roaming),
            home: Some(home),
        })
    }

    pub fn get(&self, folder: Folder) -> Option<&Path> {
        match folder {
            Folder::Home => &self.home,
            Folder::Documents => &self.documents,
            Folder::AppData => &self.appdata,
            Folder::LocalAppData => &self.local_appdata,
            Folder::Data => &self.data,
            Folder::Config => &self.config,
        }
        .as_deref()
    }

    fn path(&self, root: &RootPath) -> Option<PathBuf> {
        let folder = self.get(root.folder)?;
        Some(match &root.path {
            Some(path) => path.under(folder),
            None => folder.to_owned(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Macos,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Macos
        } else {
            Os::Linux
        }
    }
}

/// What's known about where a game's roots are on this machine, besides
/// what its definition says.
pub struct RootSetup<'a> {
    pub os: Os,
    /// The game's own folder.
    pub game: &'a Path,
    /// This machine's folders, for Windows and native builds.
    pub host: &'a Folders,
    /// The Wine prefix the game runs in, for a Windows game on another
    /// system, if there is one.
    pub prefix: Option<&'a Path>,
    /// Where the user has said roots are, which wins over everything else.
    pub set: &'a BTreeMap<RootId, PathBuf>,
}

/// How patchwork worked out where a root is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RootOrigin {
    /// The user said.
    Set,
    /// From the game's definition, on this machine.
    Definition,
    /// From the game's definition, inside its Wine prefix.
    Prefix,
}

/// One of a game's other roots, worked out for this machine.
#[derive(Debug, Clone)]
pub struct ResolvedRoot {
    /// Where it is, or why that couldn't be worked out.
    pub path: std::result::Result<PathBuf, String>,
    pub origin: RootOrigin,
    pub allow: Vec<FilePattern>,
    matcher: GlobSet,
}

/// Every root a target has, with what may go in each one.
#[derive(Debug, Clone)]
pub struct ResolvedRoots {
    game: PathBuf,
    prefix: Option<PathBuf>,
    others: BTreeMap<RootId, ResolvedRoot>,
}

impl ResolvedRoots {
    /// Works out where each of `roots` is.
    pub fn new(game: &str, roots: &RootDefs, setup: &RootSetup<'_>) -> Self {
        let others = roots
            .iter()
            .map(|(id, def)| {
                let (path, origin) = match setup.set.get(id) {
                    Some(path) => (Ok(path.clone()), RootOrigin::Set),
                    None => locate(game, id, def, setup),
                };
                (id.clone(), ResolvedRoot::new(path, origin, &def.allow))
            })
            .collect();
        Self {
            game: setup.game.to_owned(),
            prefix: setup.prefix.map(Path::to_owned),
            others,
        }
    }

    /// Just the game's own folder.
    pub fn game(path: &Path) -> Self {
        Self {
            game: path.to_owned(),
            prefix: None,
            others: BTreeMap::new(),
        }
    }

    /// The Wine prefix they were looked for in, if any.
    pub fn prefix(&self) -> Option<&Path> {
        self.prefix.as_deref()
    }

    /// Adds a root at `path` that only files matching `allow` may go in.
    pub fn with(mut self, id: RootId, path: &Path, allow: &[&str]) -> Self {
        let allow = allow
            .iter()
            .map(|a| FilePattern::try_from(a.to_string()).expect("valid pattern"))
            .collect::<Vec<_>>();
        let root = ResolvedRoot::new(Ok(path.to_owned()), RootOrigin::Definition, &allow);
        self.others.insert(id, root);
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = (&RootId, &ResolvedRoot)> {
        self.others.iter()
    }

    /// The ones that were found.
    pub fn roots(&self) -> Roots {
        self.others
            .iter()
            .filter_map(|(id, root)| Some((id, root.path.as_ref().ok()?)))
            .fold(Roots::game(&self.game), |roots, (id, path)| {
                roots.with(id.clone(), path)
            })
    }

    /// Whether a file can go to `dest`. `Err` with whether the deployer was
    /// at fault for putting it there, and why it can't.
    pub(crate) fn check(&self, dest: &Dest) -> std::result::Result<(), (bool, String)> {
        if dest.root.is_game() {
            return Ok(());
        }
        let Some(root) = self.others.get(&dest.root) else {
            return Err((
                true,
                format!(
                    "{dest} is in a folder called `{}`, which this game doesn't have",
                    dest.root
                ),
            ));
        };
        if !root.matcher.is_match(dest.path.as_str()) {
            let allow: Vec<_> = root.allow.iter().map(|a| a.as_str()).collect();
            return Err((
                true,
                format!(
                    "{dest} isn't allowed in `{}`, which can only have {}",
                    dest.root,
                    allow.join(", ")
                ),
            ));
        }
        let path = root
            .path
            .as_ref()
            .map_err(|why| (false, format!("{dest} can't be deployed because {why}")))?;
        if !path.is_dir() {
            return Err((
                false,
                format!(
                    "{dest} goes in {}, which doesn't exist; games usually make their folders the first time they run, so try starting it once",
                    path.display()
                ),
            ));
        }
        Ok(())
    }
}

impl ResolvedRoot {
    fn new(
        path: std::result::Result<PathBuf, String>,
        origin: RootOrigin,
        allow: &[FilePattern],
    ) -> Self {
        let mut set = GlobSetBuilder::new();
        for pattern in allow {
            // checked when the definition is loaded
            if let Ok(glob) = GlobBuilder::new(pattern.as_str())
                .case_insensitive(true)
                .literal_separator(true)
                .build()
            {
                set.add(glob);
            }
        }
        Self {
            path,
            origin,
            allow: allow.to_vec(),
            matcher: set.build().unwrap_or_else(|_| GlobSet::empty()),
        }
    }
}

/// Where the game definition says root `id` is, on this machine.
fn locate(
    game: &str,
    id: &RootId,
    def: &RootDef,
    setup: &RootSetup<'_>,
) -> (std::result::Result<PathBuf, String>, RootOrigin) {
    let native = match setup.os {
        Os::Windows => Some(&def.path),
        Os::Linux => def.linux.as_ref(),
        Os::Macos => def.macos.as_ref(),
    };
    let set_it = format!("set it with `patchwork game set {game} root.{id} <dir>`");
    if let Some(path) = native {
        let found = setup.host.path(path).ok_or_else(|| {
            format!(
                "your {{{}}} folder couldn't be found; {set_it}",
                path.folder
            )
        });
        return (found, RootOrigin::Definition);
    }
    if setup.os == Os::Macos {
        return (
            Err(format!(
                "the game doesn't say where its `{id}` folder is on macOS; {set_it}"
            )),
            RootOrigin::Definition,
        );
    }
    let found = match setup.prefix {
        None => Err(format!(
            "the Wine prefix the game runs in couldn't be found; Steam makes it the first time the game runs, or set it with `patchwork game set {game} prefix <dir>`"
        )),
        Some(prefix) => Folders::wine(prefix)
            .and_then(|folders| folders.path(&def.path))
            .ok_or_else(|| {
                format!(
                    "{} doesn't look like a Wine prefix (it has no `drive_c/users`)",
                    prefix.display()
                )
            }),
    };
    (found, RootOrigin::Prefix)
}
