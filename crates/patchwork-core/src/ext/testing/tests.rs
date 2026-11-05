use std::fs;

use super::*;
use crate::action::{Action, ActionContext, Outcome, StateChange};
use crate::deploy::{
    BuildContext, DeployContext, Deployer, DesiredState, Dest, InstalledMod, Placement, Src,
};
use crate::ext::Registrar;
use crate::list::{ListContext, ListItem, ListItems, OrderedList};
use crate::relpath::RelPath;

/// Puts mods' files in `Mods/`, and the `names` list's order in
/// `cfg:order.txt`.
struct Simple;

impl Deployer for Simple {
    fn kind(&self) -> &str {
        "simple"
    }

    fn plan(&self, _: &DeployContext<'_>, mods: &[InstalledMod]) -> Result<DesiredState> {
        let placements = mods
            .iter()
            .flat_map(|m| {
                m.files.iter().map(|f| Placement {
                    dst: Dest::game(RelPath::new(&format!("Mods/{}", f.rel)).unwrap()),
                    src: Src::Mod {
                        hash: m.hash,
                        rel: f.rel.clone(),
                    },
                    mod_name: m.name.clone(),
                })
            })
            .collect();
        Ok(DesiredState {
            placements,
            warnings: Vec::new(),
        })
    }

    fn builds(&self, _: &DeployContext<'_>) -> bool {
        true
    }

    fn build(
        &self,
        ctx: &DeployContext<'_>,
        build: &BuildContext<'_>,
        mods: &[InstalledMod],
    ) -> Result<DesiredState> {
        let mut built = self.plan(ctx, mods)?;
        let list = build.list("simple.names").unwrap();
        let names: Vec<&str> = list
            .items
            .iter()
            .filter(|i| i.enabled)
            .map(|i| i.item.name.as_str())
            .collect();
        fs::write(build.out.join("order.txt"), names.join("\n")).unwrap();
        let rel = RelPath::new("order.txt").unwrap();
        built.placements.push(Placement {
            dst: Dest::new(RootId::new("cfg").unwrap(), rel.clone()),
            src: Src::Built(rel),
            mod_name: "built".to_owned(),
        });
        Ok(built)
    }
}

/// Each mod's name, in the order they're in.
struct Names;

impl OrderedList for Names {
    fn title(&self) -> &str {
        "Names"
    }

    fn items(&self, _: &ListContext<'_>, mods: &[InstalledMod]) -> Result<ListItems> {
        let items = mods
            .iter()
            .map(|m| ListItem {
                name: m.name.clone(),
                mod_name: m.name.clone(),
                tier: 0,
                badges: Vec::new(),
                after: Vec::new(),
                detail: None,
                can_disable: true,
            })
            .collect();
        Ok(ListItems {
            items,
            ..Default::default()
        })
    }
}

struct SimpleExt;

impl Extension for SimpleExt {
    fn id(&self) -> &'static str {
        "simple"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(Simple).ordered_list("names", Names);
    }
}

const GAME: &str = r#"
id = "simple-game"
name = "Simple"

[deployer]
kind = "simple"

[ext.simple.names]

[roots.cfg]
path = "{localappdata}/Simple"
allow = ["*.txt"]
"#;

#[test]
fn mods_are_listed_and_deployed_into_the_temporary_game() {
    let fixture = tempfile::tempdir().unwrap();
    fs::write(fixture.path().join("b.txt"), "b").unwrap();
    let t = TestGame::from_toml(GAME)
        .extension(SimpleExt)
        .mod_files("a", [("a.txt", "a"), ("sub/a2.txt", "a2")])
        .mod_from_dir("b", fixture.path())
        .game_file("Game.exe", b"exe")
        .game_file("cfg:settings.txt", "keep");

    assert_eq!(t.order("simple.names").unwrap(), ["a", "b"]);
    assert_eq!(t.read("Game.exe").unwrap(), "exe");
    assert_eq!(t.read("cfg:settings.txt").unwrap(), "keep");
    assert!(t.root("cfg").starts_with(t.game_dir().parent().unwrap()));

    t.set_order("simple.names", &["b", "a"]).unwrap();
    let plan = t.deploy().unwrap();
    assert_eq!(
        plan.placed(),
        [
            "Mods/a.txt",
            "Mods/b.txt",
            "Mods/sub/a2.txt",
            "cfg:order.txt"
        ]
    );
    assert_eq!(plan.placed_by("Mods/b.txt"), Some("b"));
    assert_eq!(t.read("cfg:order.txt").unwrap(), "b\na");
    assert_eq!(t.read("Mods/sub/a2.txt").unwrap(), "a2");

    // a second deploy only changes what's changed
    t.set_list_enabled("simple.names", "a", false).unwrap();
    t.set_enabled("b", false).unwrap();
    let plan = t.deploy().unwrap();
    assert_eq!(plan.placed(), ["cfg:order.txt"]);
    assert_eq!(plan.removed(), ["Mods/b.txt"]);
    assert_eq!(
        t.deployed().unwrap(),
        ["Mods/a.txt", "Mods/sub/a2.txt", "cfg:order.txt"]
    );
    assert!(!t.exists("Mods/b.txt"));
}

#[test]
#[should_panic(expected = "couldn't set up the test game: unknown deployer")]
fn setup_failures_panic() {
    let t = TestGame::from_toml(GAME);
    t.patchwork();
}

#[test]
#[should_panic(expected = "before it's used")]
fn mods_are_added_before_the_game_is_used() {
    let t = TestGame::from_toml(GAME).extension(SimpleExt);
    t.patchwork();
    let _ = t.mod_files("late", [("x", "x")]);
}

/// Reverses the `simple.names` list, reading it if `read`.
struct Reverse {
    read: bool,
}

#[async_trait]
impl Action for Reverse {
    fn title(&self) -> &str {
        "Reverse"
    }

    fn shown_with(&self) -> Option<&str> {
        Some("simple.names")
    }

    async fn run(&self, ctx: &ActionContext<'_>) -> Result<Outcome> {
        let order = if self.read {
            let list = ctx.list("simple.names")?;
            list.items
                .iter()
                .rev()
                .map(|i| i.item.name.clone())
                .collect()
        } else {
            vec!["b".to_owned(), "a".to_owned()]
        };
        Ok(Outcome {
            message: Some("Reversed".to_owned()),
            changes: vec![StateChange::List {
                id: "simple.names".to_owned(),
                order,
            }],
            warnings: Vec::new(),
        })
    }
}

/// `Reverse` in extension `rev`, declaring that it uses `simple.names` if
/// `declare`.
struct RevExt {
    declare: bool,
    read: bool,
}

impl Extension for RevExt {
    fn id(&self) -> &'static str {
        "rev"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        if self.declare {
            ext.uses("simple.names");
        }
        ext.action("reverse", Reverse { read: self.read });
    }
}

fn with_rev(game: &str, declare: bool, read: bool) -> TestGame {
    TestGame::from_toml(&format!("{game}\n[ext.rev]\n"))
        .extension(SimpleExt)
        .extension(RevExt { declare, read })
        .mod_files("a", [("a.txt", "a")])
        .mod_files("b", [("b.txt", "b")])
        .mod_files("c", [("c.txt", "c")])
}

#[test]
fn actions_change_lists_they_use_only_when_applied() {
    let t = with_rev(GAME, true, true);
    let actions = t.actions().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].id, "rev.reverse");
    assert_eq!(actions[0].shown_with.as_deref(), Some("simple.names"));

    let report = t.run_action("rev.reverse", false).unwrap();
    assert!(!report.applied);
    assert_eq!(report.message.as_deref(), Some("Reversed"));
    let change = &report.lists[0];
    let after: Vec<&str> = change.after.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(after, ["c", "b", "a"]);
    assert_eq!(change.moved.len(), 2);
    assert_eq!(t.order("simple.names").unwrap(), ["a", "b", "c"]);

    t.run_action("rev.reverse", true).unwrap();
    assert_eq!(t.order("simple.names").unwrap(), ["c", "b", "a"]);
}

#[test]
fn names_left_out_keep_their_order_after_the_rest() {
    let t = with_rev(GAME, true, false);
    let report = t.run_action("rev.reverse", true).unwrap();
    assert!(report.warnings.is_empty());
    assert_eq!(t.order("simple.names").unwrap(), ["b", "a", "c"]);
}

#[test]
fn undeclared_lists_cant_be_read_or_changed() {
    let t = with_rev(GAME, false, true);
    let e = t.run_action("rev.reverse", false).unwrap_err().to_string();
    assert!(e.contains("without declaring it"), "{e}");
    let t = with_rev(GAME, false, false);
    let e = t.run_action("rev.reverse", true).unwrap_err().to_string();
    assert!(e.contains("without declaring it"), "{e}");
    assert_eq!(t.order("simple.names").unwrap(), ["a", "b", "c"]);
}

#[test]
fn actions_are_off_where_what_they_use_is_off() {
    let game = GAME.replace("[ext.simple.names]", "[ext.simple]\noff = [\"names\"]");
    let t = with_rev(&game, true, true);
    assert!(t.actions().unwrap().is_empty());
    let e = t.run_action("rev.reverse", false).unwrap_err().to_string();
    assert!(e.contains("doesn't have the `rev.reverse` action"), "{e}");
}

#[test]
fn using_what_isnt_built_in_stops_the_build() {
    let tmp = tempfile::tempdir().unwrap();
    let e = Patchwork::builder(Paths::rooted(tmp.path()))
        .extension(RevExt {
            declare: true,
            read: true,
        })
        .open()
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("uses `simple.names`, which isn't a list"), "{e}");
}
