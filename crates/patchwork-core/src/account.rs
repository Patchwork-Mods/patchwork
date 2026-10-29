//! Who the user plays as in games patchwork starts itself (see
//! [`crate::provision`]). Only Minecraft asks so far, so these are its
//! accounts, and [`Account::vars`] fills in its launch arguments.

use std::collections::BTreeMap;

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Account {
    /// Plays under a name of the user's own without signing in: fine on
    /// your own, and on servers that don't check who joins.
    Offline { name: String },
}

impl Account {
    /// An offline account named `name`, which has to be a name Minecraft
    /// accepts: 3 to 16 letters, digits and underscores.
    pub fn offline(name: &str) -> Result<Self> {
        let fits = (3..=16).contains(&name.len())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !fits {
            return Err(Error::invalid(format!(
                "`{name}` can't be a Minecraft name; it has to be 3 to 16 letters, digits and underscores"
            )));
        }
        Ok(Account::Offline {
            name: name.to_owned(),
        })
    }

    pub fn name(&self) -> &str {
        match self {
            Account::Offline { name } => name,
        }
    }

    /// The player's id, without dashes. Offline players get the one
    /// servers in offline mode give them, from their name.
    pub fn uuid(&self) -> String {
        match self {
            Account::Offline { name } => offline_uuid(name),
        }
    }

    /// The launch argument placeholders it fills in.
    pub fn vars(&self) -> BTreeMap<String, String> {
        let uuid = self.uuid();
        let (token, user_type) = match self {
            Account::Offline { .. } => ("0", "legacy"),
        };
        [
            ("auth_player_name", self.name()),
            ("auth_uuid", &uuid),
            ("auth_access_token", token),
            ("auth_session", token),
            ("auth_xuid", "0"),
            ("clientid", "0"),
            ("user_type", user_type),
            ("user_properties", "{}"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }
}

/// What Java's `UUID.nameUUIDFromBytes("OfflinePlayer:<name>")` gives,
/// which is how offline servers name players: an MD5 (version 3) UUID
/// without a namespace.
fn offline_uuid(name: &str) -> String {
    let mut hash: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}")).into();
    hash[6] = (hash[6] & 0x0f) | 0x30;
    hash[8] = (hash[8] & 0x3f) | 0x80;
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_ids_match_what_servers_give() {
        // from `UUID.nameUUIDFromBytes("OfflinePlayer:Notch".getBytes())`
        let notch = Account::offline("Notch").unwrap();
        assert_eq!(notch.uuid(), "b50ad385829d3141a2167e7d7539ba7f");
        assert_eq!(notch.vars()["auth_player_name"], "Notch");
    }

    #[test]
    fn names_minecraft_wont_take_are_refused() {
        assert!(Account::offline("ab").is_err());
        assert!(Account::offline("has space").is_err());
        assert!(Account::offline("seventeen_letters").is_err());
        assert!(Account::offline("Steve_2").is_ok());
    }
}
