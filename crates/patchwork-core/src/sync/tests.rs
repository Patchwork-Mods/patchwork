use super::*;
use crate::hash::ContentHash;
use crate::lock::mod_key;
use crate::plugins::PluginEntry;
use crate::source::{ArtifactFormat, ArtifactLocation};

fn profile_mod(id: &str) -> ProfileMod {
    ProfileMod {
        name: id.to_owned(),
        source: "thunderstore".to_owned(),
        id: id.to_owned(),
        version: None,
        enabled: true,
        game_versions: None,
        fomod: None,
    }
}

fn locked(id: &str, version: &str, deps: &[&str]) -> LockedMod {
    LockedMod {
        name: id.to_owned(),
        source: "thunderstore".to_owned(),
        id: id.to_owned(),
        requested: None,
        version: Some(version.to_owned()),
        hash: ContentHash::of(format!("{id} {version}").as_bytes()),
        format: ArtifactFormat::Archive,
        artifact: ArtifactLocation::Url {
            url: format!("https://example.com/{id}-{version}.zip"),
        },
        dependencies: deps.iter().map(|d| mod_key("thunderstore", d)).collect(),
        requires: BTreeMap::new(),
        game_versions: None,
        env: Default::default(),
    }
}

/// A machine with its own copy of the profile, and its own copy of the
/// synced entries, each with the time it was written.
struct Machine {
    profile: Profile,
    lock: Option<Lockfile>,
    base: Entries,
    replica: BTreeMap<String, (u64, Vec<u8>)>,
}

impl Machine {
    fn new(profile: Profile, lock: Option<Lockfile>) -> Self {
        Self {
            profile,
            lock,
            base: Entries::new(),
            replica: BTreeMap::new(),
        }
    }

    fn entries(&self) -> Entries {
        to_entries(&self.profile, self.lock.as_ref())
    }

    /// Writes what's changed since the last sync to its replica.
    fn publish(&mut self, clock: &mut u64) {
        for (k, v) in changes(&self.base, &self.entries()) {
            *clock += 1;
            self.replica.insert(k, (*clock, v));
        }
    }

    /// Takes what its replica says, as the newest value of each key.
    fn adopt(&mut self) {
        let entries: Entries = self
            .replica
            .iter()
            .map(|(k, (_, v))| (k.clone(), v.clone()))
            .collect();
        let synced = from_entries(&entries).unwrap();
        self.profile = synced.profile;
        self.lock = synced.lock;
        self.base = entries;
    }

    fn names(&self) -> Vec<&str> {
        self.profile.mods.iter().map(|m| m.name.as_str()).collect()
    }

    fn locked(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .lock
            .iter()
            .flat_map(|l| &l.mods)
            .map(|m| m.key())
            .collect();
        keys.sort();
        keys
    }
}

/// Last write wins for each key, in both directions, like iroh-docs.
fn merge(a: &mut Machine, b: &mut Machine) {
    for (k, (t, v)) in b.replica.clone() {
        if a.replica.get(&k).is_none_or(|(at, _)| *at < t) {
            a.replica.insert(k, (t, v));
        }
    }
    b.replica = a.replica.clone();
}

/// Two machines with the same profile of mods `ids`, all synced.
fn pair(ids: &[&str]) -> (Machine, Machine, u64) {
    let mut profile = Profile::new("my-game", "default");
    profile.mods = ids.iter().map(|id| profile_mod(id)).collect();
    let lock = Lockfile {
        schema: LOCK_SCHEMA,
        game: LockedGame {
            env: Default::default(),
            id: "my-game".to_owned(),
        },
        mods: ids.iter().map(|id| locked(id, "1.0.0", &[])).collect(),
    };
    let mut a = Machine::new(profile, Some(lock));
    let mut clock = 0;
    a.publish(&mut clock);
    a.adopt();
    let mut b = Machine::new(Profile::new("other", "other"), None);
    merge(&mut a, &mut b);
    b.adopt();
    (a, b, clock)
}

/// Each publishes, they sync, and each takes the result.
fn sync(a: &mut Machine, b: &mut Machine, clock: &mut u64) {
    a.publish(clock);
    b.publish(clock);
    merge(a, b);
    a.adopt();
    b.adopt();
}

#[test]
fn a_profile_survives_the_trip() {
    let mut profile = Profile::new("my-game", "default");
    profile.hidden_tags = vec!["library".to_owned()];
    profile.env = BTreeMap::from([("loader".to_owned(), "quilt".to_owned())]);
    profile.mods = vec![profile_mod("B"), profile_mod("A")];
    profile.mods[1].enabled = false;
    profile.mods[1].version = Some("1.0.0".to_owned());
    profile.plugins = vec![PluginEntry {
        name: "A.esp".to_owned(),
        enabled: false,
    }];
    let lock = Lockfile {
        schema: LOCK_SCHEMA,
        game: LockedGame {
            env: profile.env.clone(),
            id: "my-game".to_owned(),
        },
        mods: vec![
            locked("B", "2.0.0", &["Lib"]),
            locked("A", "1.0.0", &[]),
            locked("Lib", "1.0.0", &[]),
        ],
    };
    let entries = to_entries(&profile, Some(&lock));
    let synced = from_entries(&entries).unwrap();
    assert_eq!(synced.profile.id, profile.id);
    assert_eq!(synced.profile.name, "default");
    assert_eq!(synced.profile.game, "my-game");
    assert_eq!(synced.profile.hidden_tags, profile.hidden_tags);
    assert_eq!(synced.profile.mods, profile.mods);
    assert_eq!(synced.profile.plugins, profile.plugins);
    assert_eq!(synced.profile.env, profile.env);
    let back = synced.lock.unwrap();
    assert_eq!(back.game.env, lock.game.env);
    assert_eq!(
        serde_json::to_value(&back.mods).unwrap(),
        serde_json::to_value(&lock.mods).unwrap()
    );
    // and nothing needs writing again
    assert!(changes(&entries, &to_entries(&synced.profile, Some(&back))).is_empty());
}

#[test]
fn mods_added_at_once_on_both_machines_both_survive() {
    let (mut a, mut b, mut clock) = pair(&["Base"]);
    a.profile.mods.push(profile_mod("FromA"));
    a.lock
        .as_mut()
        .unwrap()
        .mods
        .push(locked("FromA", "1.0.0", &["LibA"]));
    a.lock
        .as_mut()
        .unwrap()
        .mods
        .push(locked("LibA", "1.0.0", &[]));
    b.profile.mods.push(profile_mod("FromB"));
    b.lock
        .as_mut()
        .unwrap()
        .mods
        .push(locked("FromB", "1.0.0", &[]));

    sync(&mut a, &mut b, &mut clock);
    for m in [&a, &b] {
        // b published last, so its order wins, and a's new mod goes after
        assert_eq!(m.names(), ["Base", "FromB", "FromA"]);
        // each one's lock came along with it, so nothing needs resolving
        assert_eq!(
            m.locked(),
            [
                "thunderstore:Base",
                "thunderstore:FromA",
                "thunderstore:FromB",
                "thunderstore:LibA"
            ]
        );
    }
}

#[test]
fn changes_to_different_mods_at_once_both_survive() {
    let (mut a, mut b, mut clock) = pair(&["One", "Two", "Three"]);
    a.profile.find_mut("One").unwrap().enabled = false;
    b.profile.find_mut("Two").unwrap().version = Some("1.0.0".to_owned());
    b.profile.mods.retain(|m| m.name != "Three");
    b.lock.as_mut().unwrap().mods.retain(|m| m.id != "Three");

    sync(&mut a, &mut b, &mut clock);
    for m in [&a, &b] {
        assert_eq!(m.names(), ["One", "Two"]);
        assert!(!m.profile.mods[0].enabled);
        assert_eq!(m.profile.mods[1].version.as_deref(), Some("1.0.0"));
        assert_eq!(m.locked(), ["thunderstore:One", "thunderstore:Two"]);
    }
    // and it settles: nothing's left to publish
    sync(&mut a, &mut b, &mut clock);
    assert!(changes(&a.base, &a.entries()).is_empty());
    assert!(changes(&b.base, &b.entries()).is_empty());
}

#[test]
fn of_two_reorders_at_once_the_later_wins() {
    let (mut a, mut b, mut clock) = pair(&["One", "Two", "Three"]);
    a.profile.mods.rotate_left(1);
    b.profile.mods.reverse();
    // a mod added while someone else reorders isn't lost
    b.profile.mods.push(profile_mod("New"));

    sync(&mut a, &mut b, &mut clock);
    for m in [&a, &b] {
        assert_eq!(m.names(), ["Three", "Two", "One", "New"]);
    }
}

#[test]
fn of_a_change_and_a_removal_of_one_mod_the_later_wins() {
    let (mut a, mut b, mut clock) = pair(&["One", "Two"]);
    b.profile.mods.retain(|m| m.name != "One");
    b.lock.as_mut().unwrap().mods.retain(|m| m.id != "One");
    a.profile.find_mut("One").unwrap().enabled = false;

    // a publishes after b
    b.publish(&mut clock);
    a.publish(&mut clock);
    merge(&mut a, &mut b);
    a.adopt();
    b.adopt();
    for m in [&a, &b] {
        assert_eq!(m.names(), ["Two", "One"]);
        assert!(!m.profile.mods[1].enabled);
        // b's removal of its lock went through, though, as a didn't
        // change that, so it has to be locked again
        assert_eq!(m.locked(), ["thunderstore:Two"]);
        assert_eq!(unlocked(&m.profile, m.lock.as_ref()), ["One"]);
    }
}

#[test]
fn two_mods_given_one_name_at_once_are_told_apart() {
    let (mut a, mut b, mut clock) = pair(&[]);
    let mut from_a = profile_mod("A");
    from_a.name = "Cool".to_owned();
    a.profile.mods.push(from_a);
    let mut from_b = profile_mod("B");
    from_b.name = "Cool".to_owned();
    b.profile.mods.push(from_b);

    sync(&mut a, &mut b, &mut clock);
    sync(&mut a, &mut b, &mut clock);
    assert_eq!(a.names(), ["Cool", "Cool-2"]);
    assert_eq!(a.names(), b.names());
}

#[test]
fn locked_dependencies_nothing_needs_are_left_out() {
    let mut profile = Profile::new("my-game", "default");
    profile.mods = vec![profile_mod("A")];
    let lock = Lockfile {
        schema: LOCK_SCHEMA,
        game: LockedGame {
            env: Default::default(),
            id: "my-game".to_owned(),
        },
        mods: vec![locked("A", "1.0.0", &["Lib"]), locked("Lib", "1.0.0", &[])],
    };
    let mut entries = to_entries(&profile, Some(&lock));
    // A was relocked elsewhere to a version without the dependency
    entries.insert(
        "lock/thunderstore:A".to_owned(),
        json(&locked("A", "2.0.0", &[])),
    );
    let synced = from_entries(&entries).unwrap();
    let mods = synced.lock.unwrap().mods;
    assert_eq!(mods.len(), 1);
    assert_eq!(mods[0].version.as_deref(), Some("2.0.0"));
}

#[test]
fn newer_formats_are_refused() {
    let mut entries = to_entries(&Profile::new("my-game", "default"), None);
    entries.insert(
        FORMAT.to_owned(),
        json(&FormatEntry {
            format: SYNC_FORMAT + 1,
        }),
    );
    let err = from_entries(&entries).unwrap_err();
    assert!(err.to_string().contains("newer patchwork"), "{err}");

    // but things it doesn't know about are fine
    let mut entries = to_entries(&Profile::new("my-game", "default"), None);
    entries.insert("something-new".to_owned(), b"{}".to_vec());
    entries.insert(
        "mod/thunderstore:A".to_owned(),
        br#"{"name":"A","source":"thunderstore","id":"A","new_field":1}"#.to_vec(),
    );
    assert_eq!(from_entries(&entries).unwrap().profile.mods.len(), 1);

    let err = from_entries(&Entries::new()).unwrap_err();
    assert!(err.to_string().contains("hasn't arrived"), "{err}");
}
