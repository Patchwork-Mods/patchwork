//! Whether a package manager (Homebrew, the AUR, Scoop, a Linux distro, ...)
//! looks after this copy of patchwork. Such a copy never replaces itself:
//! the app and `patchwork self-update` say how to update it there instead,
//! and the app doesn't check for new releases on its own.
//!
//! A package says so in one of two ways:
//!
//! - When building from source, by setting `PATCHWORK_PACKAGE_MANAGER` (and
//!   optionally `PATCHWORK_UPDATE_COMMAND`) in the build's environment.
//! - When repackaging our own builds, with a `packaging.toml` beside the
//!   executable or in `<prefix>/share/patchwork/` (for an executable in
//!   `<prefix>/bin/`):
//!
//! ```toml
//! # Who updates this copy, as in "Patchwork was installed with ...".
//! manager = "the AUR"
//! # Optional: the command that updates it, shown with a copy button.
//! update-command = "yay -Syu patchwork-bin"
//! # Optional: let the app check for new releases by itself (off by
//! # default, since a package usually arrives later than the release).
//! check-automatically = false
//! ```
//!
//! The build's environment wins over a file.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// The file a package puts beside the executable to say it manages it.
pub const FILE: &str = "packaging.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "kebab-case", serialize = "camelCase"))]
pub struct Packaging {
    /// Who installed this copy, such as "Homebrew" or "the AUR".
    pub manager: String,
    /// The command that updates it, if there's one to suggest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_command: Option<String>,
    /// Whether the app may check for new releases without being asked.
    #[serde(default)]
    pub check_automatically: bool,
}

impl Packaging {
    /// Tells someone how to update this copy, for the CLI and errors.
    pub fn how_to_update(&self) -> String {
        let mut how = format!(
            "patchwork was installed with {}, so update it there",
            self.manager
        );
        if let Some(command) = &self.update_command {
            how.push_str(&format!(" (`{command}`)"));
        }
        how
    }
}

/// How this copy was packaged, if a package manager looks after it. Looked
/// up once, since it can't change while patchwork runs.
pub fn packaging() -> Option<&'static Packaging> {
    static FOUND: LazyLock<Option<Packaging>> = LazyLock::new(|| {
        built_with(
            option_env!("PATCHWORK_PACKAGE_MANAGER"),
            option_env!("PATCHWORK_UPDATE_COMMAND"),
        )
        .or_else(|| {
            let exe = std::env::current_exe().ok()?;
            candidates(&exe).iter().find_map(|path| read(path))
        })
    });
    FOUND.as_ref()
}

/// What the build's environment said, if it named a package manager.
fn built_with(manager: Option<&str>, command: Option<&str>) -> Option<Packaging> {
    let manager = manager.map(str::trim).filter(|m| !m.is_empty())?;
    Some(Packaging {
        manager: manager.to_owned(),
        update_command: command
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_owned),
        check_automatically: false,
    })
}

/// Where a `packaging.toml` for `exe` may be, in the order they're tried:
/// beside it and in `../share/patchwork/`, both for `exe` as given and for
/// where it really is if that's through a link.
fn candidates(exe: &Path) -> Vec<PathBuf> {
    let mut exes = vec![exe.to_owned()];
    if let Ok(real) = exe.canonicalize()
        && real != exe
    {
        exes.push(real);
    }
    let mut found = Vec::new();
    for exe in exes {
        let Some(dir) = exe.parent() else { continue };
        found.push(dir.join(FILE));
        if let Some(prefix) = dir.parent() {
            found.push(prefix.join("share/patchwork").join(FILE));
        }
    }
    found
}

fn read(path: &Path) -> Option<Packaging> {
    let text = std::fs::read_to_string(path).ok()?;
    match parse(&text) {
        Ok(found) => Some(found),
        Err(e) => {
            // A package that tried to say it manages patchwork but got it
            // wrong; carry on as if it hadn't, but say why.
            eprintln!("ignoring {}: {e}", path.display());
            None
        }
    }
}

fn parse(text: &str) -> Result<Packaging, String> {
    let found: Packaging = toml::from_str(text).map_err(|e| e.to_string())?;
    if found.manager.trim().is_empty() {
        return Err("`manager` is empty".to_owned());
    }
    Ok(Packaging {
        manager: found.manager.trim().to_owned(),
        update_command: found
            .update_command
            .map(|c| c.trim().to_owned())
            .filter(|c| !c.is_empty()),
        ..found
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_environment() {
        assert_eq!(built_with(None, Some("brew upgrade")), None);
        assert_eq!(built_with(Some("  "), None), None);
        assert_eq!(
            built_with(Some("Homebrew"), Some(" ")),
            Some(Packaging {
                manager: "Homebrew".to_owned(),
                update_command: None,
                check_automatically: false,
            })
        );
    }

    #[test]
    fn file() {
        let found = parse(
            "manager = \"the AUR\"\nupdate-command = \"yay -Syu patchwork-bin\"\n\
             check-automatically = true\n",
        )
        .unwrap();
        assert_eq!(found.manager, "the AUR");
        assert_eq!(
            found.update_command.as_deref(),
            Some("yay -Syu patchwork-bin")
        );
        assert!(found.check_automatically);
        let found = parse("manager = \"Scoop\"\nupdate-command = \"\"\n").unwrap();
        assert_eq!(found.update_command, None);
        assert!(!found.check_automatically);
        assert!(parse("manager = \" \"").is_err());
        assert!(parse("update-command = \"x\"").is_err());
    }

    #[test]
    fn looks_beside_the_exe_and_in_share() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("usr/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("patchwork");
        std::fs::write(&exe, "").unwrap();
        let exe = exe.canonicalize().unwrap();
        let usr = exe.parent().unwrap().parent().unwrap();
        assert_eq!(
            candidates(&exe),
            [
                usr.join("bin").join(FILE),
                usr.join("share/patchwork").join(FILE)
            ]
        );
        std::fs::create_dir_all(usr.join("share/patchwork")).unwrap();
        std::fs::write(
            usr.join("share/patchwork").join(FILE),
            "manager = \"Arch Linux\"",
        )
        .unwrap();
        let found = candidates(&exe).iter().find_map(|p| read(p)).unwrap();
        assert_eq!(found.manager, "Arch Linux");
    }
}
