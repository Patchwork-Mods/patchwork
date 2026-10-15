//! Built-in profile formats, for exporting and importing profiles.

mod archive;
mod r2modman;
mod urls;

pub use archive::Archive;
pub use r2modman::{R2modman, ThunderstoreCode};
pub use urls::UrlList;

use std::io::{Cursor, Read};

use patchwork_core::Error;

fn error(format_id: &str, message: impl Into<String>) -> Error {
    Error::Format {
        format_id: format_id.to_owned(),
        message: message.into(),
    }
}

/// Whether `input` is a zip with an entry called `name`.
fn zip_contains(input: &[u8], name: &str) -> bool {
    zip::ZipArchive::new(Cursor::new(input)).is_ok_and(|zip| zip.index_for_name(name).is_some())
}

fn read_zip_entry(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Option<String>, String> {
    let mut entry = match zip.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|e| format!("{name}: {e}"))?;
    Ok(Some(text))
}
