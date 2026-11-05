use async_trait::async_trait;
use patchwork_core::action::{ActionContext, Fetch, StateChange};
use patchwork_core::deploy::{DeployState, GameFiles, Roots};
use patchwork_core::game::GameDef;
use patchwork_core::list::{ListItem, OrderedItem};

use super::*;

const MASTERLIST: &str = r#"
prelude:
  common:
    - &say
      type: say
      content: 'Hello {0}'
common:
  - &late Late
groups:
  - name: &early Early
  - name: default
    after: [ *early ]
  - name: *late
    after: [ default ]
plugins:
  - name: 'Fix.esp'
    group: *early
    msg:
      - <<: *say
        subs: [ 'there' ]
  - name: 'Patch.esp'
    after:
      - 'Big.esp'
      - name: 'Other.esp'
        condition: 'active("Other.esp")'
    req: [ 'Needed.esp' ]
  - name: 'Overrides.*\.esp'
    group: *late
  - name: 'unknown group.esp'
    group: Nonexistent
"#;

fn item(name: &str, tier: u8, after: &[&str]) -> OrderedItem {
    OrderedItem {
        item: ListItem {
            name: name.to_owned(),
            mod_name: "m".to_owned(),
            tier,
            badges: Vec::new(),
            after: after.iter().map(|s| (*s).to_owned()).collect(),
            detail: None,
            can_disable: true,
        },
        enabled: true,
    }
}

fn list(items: Vec<OrderedItem>) -> List {
    List {
        id: load_order(),
        title: "Plugins".to_owned(),
        description: None,
        tiers: Vec::new(),
        items,
        fixed: Vec::new(),
        problems: Vec::new(),
    }
}

fn masterlist() -> Masterlist {
    Masterlist::parse(MASTERLIST.as_bytes()).unwrap()
}

#[test]
fn rules_then_groups_then_the_order_it_was() {
    let l = list(vec![
        item("Master.esm", 0, &[]),
        item("Overrides - Mine.esp", 1, &[]),
        item("Patch.esp", 1, &["Master.esm"]),
        item("Big.esp", 1, &[]),
        item("Needed.esp", 1, &[]),
        item("Plain.esp", 1, &[]),
        item("fix.ESP", 1, &[]),
        item("Other.esp", 1, &[]),
    ]);
    let sorted = sort(&l, &masterlist());
    assert_eq!(
        sorted.order,
        [
            "Master.esm",
            "fix.ESP",
            "Big.esp",
            "Needed.esp",
            "Patch.esp",
            "Plain.esp",
            "Other.esp",
            "Overrides - Mine.esp",
        ]
    );
    assert_eq!(sorted.known, 3);
    assert!(sorted.warnings.is_empty());
}

#[test]
fn masters_beat_groups() {
    // Fix.esp is early, but needs Plain.esp, so Plain.esp goes first.
    let l = list(vec![
        item("Plain.esp", 1, &[]),
        item("Fix.esp", 1, &["Plain.esp"]),
    ]);
    assert_eq!(sort(&l, &masterlist()).order, ["Plain.esp", "Fix.esp"]);
    let l = list(vec![item("Plain.esp", 1, &[]), item("Fix.esp", 1, &[])]);
    assert_eq!(sort(&l, &masterlist()).order, ["Fix.esp", "Plain.esp"]);
}

#[test]
fn tiers_are_sorted_apart_and_loops_are_broken() {
    let l = list(vec![
        item("a.esp", 1, &["b.esp"]),
        item("b.esp", 1, &["a.esp"]),
        item("c.esm", 0, &["a.esp"]),
    ]);
    let sorted = sort(&l, &Masterlist::default());
    assert_eq!(sorted.order, ["c.esm", "a.esp", "b.esp"]);
    assert_eq!(sorted.warnings.len(), 1);
    assert!(sorted.warnings[0].starts_with("a.esp is in a loop"));
}

struct Fake(&'static str);

#[async_trait]
impl Fetch for Fake {
    async fn fetch(&self, url: &str) -> Result<(Vec<u8>, Option<String>)> {
        assert_eq!(url, "https://example.com/masterlist.yaml");
        Ok((self.0.as_bytes().to_vec(), Some("old copy".to_owned())))
    }
}

#[tokio::test]
async fn the_action_reads_the_load_order_and_reorders_it() {
    let game: GameDef = toml::from_str(
        r#"
        schema = 1
        id = "g"
        name = "G"
        [deployer]
        kind = "bethesda"
        "#,
    )
    .unwrap();
    let config: toml::Table =
        toml::from_str(r#"masterlist = "https://example.com/masterlist.yaml""#).unwrap();
    let root = std::path::PathBuf::new();
    let state = DeployState::empty("g", &Roots::game(&root));
    let files = GameFiles::new(&root, &state);
    let lists = [list(vec![
        item("Plain.esp", 1, &[]),
        item("Fix.esp", 1, &[]),
    ])];
    let uses = ["bethesda.plugins"];
    let fetch = Fake(MASTERLIST);
    let ctx = ActionContext::new(
        &game,
        &config,
        LOOT_EXTENSION,
        &uses,
        &files,
        &lists,
        &[],
        &fetch,
    );
    let outcome = LootSort.run(&ctx).await.unwrap();
    assert_eq!(
        outcome.changes,
        [StateChange::List {
            id: "bethesda.plugins".to_owned(),
            order: vec!["Fix.esp".to_owned(), "Plain.esp".to_owned()],
        }]
    );
    assert_eq!(
        outcome.message.as_deref(),
        Some("Sorted 2 plugins with LOOT's masterlist, which has rules for 1 of them")
    );

    let empty = toml::Table::new();
    let ctx = ActionContext::new(
        &game,
        &empty,
        LOOT_EXTENSION,
        &uses,
        &files,
        &lists,
        &[],
        &fetch,
    );
    let e = LootSort.run(&ctx).await.unwrap_err().to_string();
    assert!(e.contains("no `masterlist`"), "{e}");
}

/// Parses the real masterlist, which is too big to keep here:
/// `PATCHWORK_LOOT_MASTERLIST=path/to/masterlist.yaml cargo test -p
/// patchwork-bethesda -- --ignored`.
#[test]
#[ignore]
fn the_real_masterlist_parses() {
    let path = std::env::var("PATCHWORK_LOOT_MASTERLIST").unwrap();
    let yaml = std::fs::read(path).unwrap();
    let m = Masterlist::parse(&yaml).unwrap();
    assert!(m.ranks.len() > 10, "{}", m.ranks.len());
    assert!(m.exact.len() > 1000, "{}", m.exact.len());
    let ussep = m
        .entry("Unofficial Skyrim Special Edition Patch.esp")
        .unwrap();
    assert!(ussep.group.is_some());
    assert!(m.entry("ccbgssse001-fish.esm").is_some());
    eprintln!("{} regexes", m.regexes.len());
}
