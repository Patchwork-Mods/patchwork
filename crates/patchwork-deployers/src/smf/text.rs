//! The game's two encrypted text files that deploying changes:
//! `Runtime/packagedefinition.txt`, which says which archives to load, and
//! `Retail/thumbs.dat`, the game's startup settings.

use rpkg_rs::encryption::xtea::Xtea;

use super::manifest::PackageDefinitionEntry;

/// The patch level every partition gets, high enough to load the
/// `patch300` archives deploying writes. The same as SMF uses.
pub const PATCH_LEVEL: u32 = 310;

pub fn decrypt(bytes: &[u8]) -> Result<String, String> {
    let text = Xtea::decrypt_text_file(bytes).map_err(|e| e.to_string())?;
    Ok(text.trim_end_matches('\0').to_owned())
}

pub fn encrypt(text: String) -> Result<Vec<u8>, String> {
    Xtea::encrypt_woa_text_file(text).map_err(|e| e.to_string())
}

/// Raises every partition's patch level and applies mods' changes, in load
/// order. Like SMF, entries for a partition go straight after its line.
pub fn patch_package_definition(
    text: &str,
    entries: &[PackageDefinitionEntry],
) -> Result<String, String> {
    let mut lines: Vec<String> = text.lines().map(raise_patch_level).collect();
    for entry in entries {
        match entry {
            PackageDefinitionEntry::Partition {
                name,
                parent,
                partition_type,
            } => {
                lines.push(String::new());
                lines.push(format!(
                    "@partition name={name} parent={parent} type={partition_type} patchlevel={PATCH_LEVEL}"
                ));
            }
            PackageDefinitionEntry::Entity { partition, path } => {
                if lines.iter().any(|l| l.contains(path.as_str())) {
                    continue;
                }
                let prefix = format!("@partition name={partition} ");
                let at = lines
                    .iter()
                    .position(|l| l.starts_with(&prefix))
                    .ok_or_else(|| {
                        format!("there's no partition `{partition}` to add {path} to")
                    })?;
                lines.insert(at + 1, path.clone());
            }
        }
    }
    // Trailing blank lines like SMF's, so the padding XTEA adds lands on
    // them rather than a partition.
    Ok(lines.join("\r\n") + "\r\n\r\n\r\n\r\n")
}

fn raise_patch_level(line: &str) -> String {
    let Some(at) = line.find("patchlevel=") else {
        return line.to_owned();
    };
    let digits = &line[at + "patchlevel=".len()..];
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    format!("{}patchlevel={PATCH_LEVEL}{}", &line[..at], &digits[end..])
}

/// Whether `text` is a package definition deploying wrote, rather than the
/// game's own.
pub fn is_patched(text: &str) -> bool {
    text.contains(&format!("patchlevel={PATCH_LEVEL}"))
}

/// Adds mods' lines to the `[Hitman5]` section. As in SMF, each goes at the
/// top of the section, so later mods' lines end up first.
pub fn patch_thumbs(text: &str, add: &[String]) -> Result<String, String> {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let at = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case("[Hitman5]"))
        .ok_or("thumbs.dat has no [Hitman5] section")?;
    for line in add {
        lines.insert(at + 1, line.clone());
    }
    Ok(lines.join("\n"))
}
