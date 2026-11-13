//! Mods' changelogs for the UI: what an update would bring in, shown before
//! it's made, and a mod's whole history.
//!
//! Changelogs are markdown written by mod authors, so they're turned into
//! HTML here with anything that could run or navigate the app taken out: raw
//! HTML is shown as text, and only http(s) links are kept (images become
//! links to them). ui/changelog.js opens the links in the browser.

use futures_util::{StreamExt, stream};
use jiff::{SignedDuration, Timestamp};
use patchwork_core::changelog::ChangelogEntry;
use patchwork_core::lock::mod_key;
use patchwork_core::{Patchwork, newer_version};
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd, html};
use serde::Serialize;
use tauri::State;

use crate::{App, CmdResult, msg, profile_ref};

/// How many mods' versions and changelogs a preview fetches at once.
const PREVIEW_CONCURRENCY: usize = 8;

/// How recently a mod's versions must have been fetched for a preview to
/// trust them rather than fetch them again, as when opening the profile has
/// just refreshed them.
const PREVIEW_MAX_AGE: SignedDuration = SignedDuration::from_mins(5);

/// One version's notes, ready to show.
#[derive(Serialize)]
pub struct UiEntry {
    version: Option<String>,
    html: String,
}

fn ui_entries(entries: Vec<ChangelogEntry>) -> Vec<UiEntry> {
    entries
        .into_iter()
        .map(|e| UiEntry {
            version: e.version,
            html: render_markdown(&e.notes),
        })
        .collect()
}

/// A mod that updating would move to a newer version, and what that
/// brings in.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingUpdate {
    key: String,
    name: String,
    from: Option<String>,
    to: String,
    changes: Vec<UiEntry>,
    /// The mod's page, for when its changelog has nothing to show.
    url: Option<String>,
    /// Why the changelog couldn't be fetched, if it couldn't. The update
    /// can still be made.
    changelog_error: Option<String>,
}

#[derive(Serialize)]
pub struct UpdatePreview {
    updates: Vec<PendingUpdate>,
    /// Mods whose versions couldn't be checked.
    failures: Vec<String>,
}

/// What updating the profile would do, for the user to confirm before
/// `update_mods` does it: each locked mod that follows the latest version
/// (dependencies included) and has a newer one out, with the changelog
/// entries in between. Only the mods in `keys` are checked, if given.
///
/// Versions are fetched unless they were just fetched (see
/// [`PREVIEW_MAX_AGE`]), which also refreshes what the profile shows.
#[tauri::command]
pub async fn preview_updates(
    app: State<'_, App>,
    id: String,
    keys: Option<Vec<String>>,
) -> CmdResult<UpdatePreview> {
    let pw = &app.pw;
    let r = profile_ref(&id)?;
    let loaded = pw.load_profile(&r).map_err(msg)?;
    let lookup = pw.lookup(&loaded).map_err(msg)?;
    let game = loaded.game;
    let Some(lock) = pw.load_lock(&r).map_err(msg)? else {
        return Ok(UpdatePreview {
            updates: Vec::new(),
            failures: Vec::new(),
        });
    };
    let since = Timestamp::now() - PREVIEW_MAX_AGE;
    let candidates: Vec<_> = lock
        .mods
        .into_iter()
        .filter(|m| m.requested.is_none() && pw.lists_versions(&m.source).unwrap_or(false))
        .filter(|m| keys.as_ref().is_none_or(|keys| keys.contains(&m.key())))
        .collect();
    let checked: Vec<Result<Option<PendingUpdate>, String>> = stream::iter(candidates)
        .map(|m| {
            let (game, lookup) = (&game, &lookup);
            async move {
                let versions = pw
                    .versions_since(lookup, &m.source, &m.id, since)
                    .await
                    .map_err(|e| format!("{}: {e}", m.name))?;
                let Some(newest) = newer_version(&versions, m.version.as_deref()) else {
                    return Ok(None);
                };
                let newest = newest.to_owned();
                let changes = pw
                    .changes(
                        lookup,
                        &m.source,
                        &m.id,
                        m.version.as_deref(),
                        Some(&newest),
                    )
                    .await;
                let key = m.key();
                let (changes, changelog_error) = match changes {
                    Ok(changes) => (ui_entries(changes), None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                };
                Ok(Some(PendingUpdate {
                    url: pw.mod_url(game, &m.source, &m.id),
                    key,
                    name: m.name,
                    from: m.version,
                    to: newest,
                    changes,
                    changelog_error,
                }))
            }
        })
        .buffered(PREVIEW_CONCURRENCY)
        .collect()
        .await;
    let mut preview = UpdatePreview {
        updates: Vec::new(),
        failures: Vec::new(),
    };
    for result in checked {
        match result {
            Ok(Some(update)) => preview.updates.push(update),
            Ok(None) => {}
            Err(failure) => preview.failures.push(failure),
        }
    }
    Ok(preview)
}

/// A mod's whole changelog, newest first, by its key (`<source>:<id>`).
#[tauri::command]
pub async fn mod_changelog(
    app: State<'_, App>,
    id: String,
    key: String,
) -> CmdResult<Vec<UiEntry>> {
    let pw: &Patchwork = &app.pw;
    let profile = pw.load_profile(&profile_ref(&id)?).map_err(msg)?;
    let m = profile
        .mods
        .iter()
        .find(|m| mod_key(&m.source, &m.id) == key)
        .ok_or_else(|| format!("{} has no mod {key}", profile.label()))?;
    let entries = pw
        .changelog(&pw.lookup(&profile).map_err(msg)?, &m.source, &m.id, None)
        .await
        .map_err(msg)?;
    Ok(ui_entries(entries))
}

/// Markdown as HTML that's safe to put in the page; see the module docs.
pub fn render_markdown(markdown: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    // whether each open link or image was kept, so its end is dropped too
    let mut kept = Vec::new();
    let events = Parser::new_ext(markdown, options).filter_map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Some(Event::Text(raw)),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        })
        | Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let safe = is_web_link(&dest_url);
            kept.push(safe);
            safe.then_some(Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }))
        }
        Event::End(TagEnd::Link | TagEnd::Image) => kept
            .pop()
            .unwrap_or(false)
            .then_some(Event::End(TagEnd::Link)),
        event => Some(event),
    });
    let mut out = String::new();
    html::push_html(&mut out, events);
    out
}

fn is_web_link(url: &CowStr) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::render_markdown;

    #[test]
    fn renders_markdown() {
        assert_eq!(
            render_markdown("### Added\n- A **thing**"),
            "<h3>Added</h3>\n<ul>\n<li>A <strong>thing</strong></li>\n</ul>\n"
        );
    }

    #[test]
    fn raw_html_is_shown_as_text() {
        let html = render_markdown("<img src=x onerror=alert(1)> and <b>bold</b>");
        assert!(!html.contains("<img"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
        assert!(html.contains("&lt;b&gt;bold&lt;/b&gt;"), "{html}");
    }

    #[test]
    fn only_web_links_are_kept() {
        assert_eq!(
            render_markdown("[site](https://example.com) [bad](javascript:alert(1))"),
            "<p><a href=\"https://example.com\">site</a> bad</p>\n"
        );
    }

    #[test]
    fn images_become_links() {
        assert_eq!(
            render_markdown("![a picture](https://example.com/a.png) ![x](data:abc)"),
            "<p><a href=\"https://example.com/a.png\">a picture</a> x</p>\n"
        );
    }
}
