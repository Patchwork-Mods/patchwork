//! Starting a game patchwork set up itself, with Java (see
//! [`crate::provision::JavaLaunch`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{Launch, Via};
use crate::account::Account;
use crate::deploy::Os;
use crate::game::LaunchDefs;
use crate::provision::JavaLaunch;
use crate::{Error, Result};

/// How much memory the game gets if its arguments don't say, as Mojang's
/// launcher gives it.
const DEFAULT_MEMORY: &str = "-Xmx2G";

/// What a [`JavaLaunch`] is started with, besides the setup itself.
pub struct JavaContext<'a> {
    pub os: Os,
    /// The folder the setup's files were downloaded to.
    pub runtime: &'a Path,
    /// The folder the game runs in, with its mods and saves.
    pub instance: &'a Path,
    pub account: &'a Account,
    /// Java, if the setup doesn't come with one.
    pub system_java: Option<PathBuf>,
    /// What gives programs the usual Linux file layout, where there isn't
    /// one (see [`super::Setup::fhs`]). Downloaded Java needs it, and so
    /// do the game's native libraries, whichever Java runs them.
    pub fhs: Option<PathBuf>,
}

/// The command that starts `java` as `ctx` says.
pub fn plan(name: &str, java: &JavaLaunch, ctx: &JavaContext) -> Result<Launch> {
    let exe = match &java.java_exe {
        Some(exe) => exe.under(ctx.runtime),
        None => ctx.system_java.clone().ok_or_else(|| {
            let version = java.java.map_or(String::new(), |v| format!(" {v}"));
            Error::invalid(format!(
                "{name} needs Java{version}, and patchwork couldn't find it; install it, or set the game up again with `patchwork provision` so it's downloaded"
            ))
        })?,
    };
    let separator = java.vars.get("classpath_separator").map_or(
        match ctx.os {
            Os::Windows => ";",
            _ => ":",
        },
        String::as_str,
    );
    let classpath = java
        .classpath
        .iter()
        .map(|p| p.under(ctx.runtime).to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(separator);

    let mut vars = java.vars.clone();
    for (key, dir) in &java.dirs {
        vars.insert(
            key.clone(),
            dir.under(ctx.runtime).to_string_lossy().into_owned(),
        );
    }
    vars.extend(ctx.account.vars());
    vars.insert("classpath".to_owned(), classpath);
    vars.insert(
        "game_directory".to_owned(),
        ctx.instance.to_string_lossy().into_owned(),
    );
    vars.insert("launcher_name".to_owned(), "patchwork".to_owned());
    vars.insert(
        "launcher_version".to_owned(),
        env!("CARGO_PKG_VERSION").to_owned(),
    );

    let mut args = Vec::new();
    if !java.jvm_args.iter().any(|a| a.starts_with("-Xmx")) {
        args.push(DEFAULT_MEMORY.to_owned());
    }
    args.extend(java.jvm_args.iter().map(|a| fill(a, &vars)));
    args.push(java.main_class.clone());
    args.extend(java.game_args.iter().map(|a| fill(a, &vars)));

    let (program, args) = match &ctx.fhs {
        Some(fhs) => {
            args.insert(0, exe.to_string_lossy().into_owned());
            (fhs.clone(), args)
        }
        None => (exe.clone(), args),
    };
    Ok(Launch {
        option: LaunchDefs::GAME.to_owned(),
        via: Via::Java { java: exe },
        program,
        args,
        env: BTreeMap::new(),
        cwd: ctx.instance.to_owned(),
        prefix_overrides: None,
    })
}

/// `arg` with its `${name}` placeholders that are in `vars` filled in.
/// Others are left as they are.
fn fill(arg: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match vars.get(name) {
                    Some(value) => out.push_str(value),
                    None => out.push_str(&rest[start..start + 2 + end + 1]),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The system's Java, from `PATH`: `javaw` on Windows, so the game doesn't
/// open a console.
pub fn system_java(os: Os) -> Option<PathBuf> {
    let name = match os {
        Os::Windows => "javaw.exe",
        _ => "java",
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relpath::RelPath;

    fn launch() -> JavaLaunch {
        JavaLaunch {
            java: Some(21),
            java_exe: Some(RelPath::new("java/bin/java").unwrap()),
            main_class: "net.minecraft.client.main.Main".to_owned(),
            classpath: vec![
                RelPath::new("libraries/a.jar").unwrap(),
                RelPath::new("versions/1.21.1/1.21.1.jar").unwrap(),
            ],
            jvm_args: vec![
                "-Djava.library.path=${natives_directory}".to_owned(),
                "-cp".to_owned(),
                "${classpath}".to_owned(),
            ],
            game_args: vec![
                "--username".to_owned(),
                "${auth_player_name}".to_owned(),
                "--gameDir".to_owned(),
                "${game_directory}".to_owned(),
                "--version".to_owned(),
                "${version_name}".to_owned(),
                "--quickPlay".to_owned(),
                "${quickPlayPath}".to_owned(),
            ],
            vars: BTreeMap::from([("version_name".to_owned(), "1.21.1".to_owned())]),
            dirs: BTreeMap::from([(
                "natives_directory".to_owned(),
                RelPath::new("natives/1.21.1").unwrap(),
            )]),
        }
    }

    #[test]
    fn placeholders_are_filled_and_unknown_ones_left() {
        let account = Account::offline("Steve").unwrap();
        let ctx = JavaContext {
            os: Os::Linux,
            runtime: Path::new("/rt"),
            instance: Path::new("/inst"),
            account: &account,
            system_java: None,
            fhs: None,
        };
        let launch = plan("Minecraft", &launch(), &ctx).unwrap();
        assert_eq!(launch.program, Path::new("/rt/java/bin/java"));
        assert_eq!(launch.cwd, Path::new("/inst"));
        assert_eq!(
            launch.args,
            [
                "-Xmx2G",
                "-Djava.library.path=/rt/natives/1.21.1",
                "-cp",
                "/rt/libraries/a.jar:/rt/versions/1.21.1/1.21.1.jar",
                "net.minecraft.client.main.Main",
                "--username",
                "Steve",
                "--gameDir",
                "/inst",
                "--version",
                "1.21.1",
                "--quickPlay",
                "${quickPlayPath}",
            ]
        );
    }

    #[test]
    fn java_runs_inside_the_fhs_wrapper_and_the_system_one_is_the_fallback() {
        let account = Account::offline("Steve").unwrap();
        let mut java = launch();
        java.java_exe = None;
        java.jvm_args.push("-Xmx4G".to_owned());
        let mut ctx = JavaContext {
            os: Os::Linux,
            runtime: Path::new("/rt"),
            instance: Path::new("/inst"),
            account: &account,
            system_java: None,
            fhs: Some(PathBuf::from("/bin/steam-run")),
        };
        let err = plan("Minecraft", &java, &ctx).unwrap_err();
        assert!(err.to_string().contains("needs Java 21"), "{err}");

        ctx.system_java = Some(PathBuf::from("/usr/bin/java"));
        let launch = plan("Minecraft", &java, &ctx).unwrap();
        assert_eq!(launch.program, Path::new("/bin/steam-run"));
        assert_eq!(
            launch.args[..2],
            ["/usr/bin/java", "-Djava.library.path=/rt/natives/1.21.1"]
        );
        assert_eq!(
            launch.via,
            Via::Java {
                java: PathBuf::from("/usr/bin/java")
            }
        );
    }

    #[test]
    fn filling_leaves_unclosed_placeholders_alone() {
        let vars = BTreeMap::from([("a".to_owned(), "1".to_owned())]);
        assert_eq!(fill("x${a}y${b}z${a", &vars), "x1y${b}z${a");
    }
}
