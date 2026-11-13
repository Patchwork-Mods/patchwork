//! File name patterns, which pick a mod's file out of each release's
//! (GitHub) or out of its files list (Nexus).
//!
//! A pattern is a file name in which `*` stands for any run of characters,
//! like `ModName-*.zip`. Matching ignores ASCII case.

/// The version in a release's tag: whatever comes from its first digit on,
/// so `v1.2.3` and `release-1.2.3` are both `1.2.3`. `None` if there's no
/// digit in it.
pub fn tag_version(tag: &str) -> Option<&str> {
    let start = tag.find(|c: char| c.is_ascii_digit())?;
    Some(&tag[start..])
}

/// Splits off a file name's extension, counting `.tar.gz` and the like as
/// one, so a version at the end of the name isn't mistaken for part of it.
fn split_extension(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    for double in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"] {
        if lower.ends_with(double) && lower.len() > double.len() {
            return name.split_at(name.len() - double.len());
        }
    }
    match name.rfind('.') {
        // a leading dot is part of the name, and a "digits only" extension
        // is more likely the end of a version than a file type
        Some(dot) if dot > 0 && name[dot + 1..].chars().any(|c| c.is_ascii_alphabetic()) => {
            name.split_at(dot)
        }
        _ => (name, ""),
    }
}

/// Guesses the pattern that picks the same file out of later releases, from
/// `name`, the file the user picked out of the release tagged `tag`:
///
/// - the tag's version in the name becomes `*`: `ModName-0.1.2.zip` from
///   `v0.1.2` is `ModName-*.zip`;
/// - failing that, any dotted numbers do, like `1.4.2` in `Mod-1.4.2.zip`
///   from a release tagged `release-7`;
/// - and a name with neither, like `ModName.zip`, is matched exactly.
pub fn infer(name: &str, tag: &str) -> String {
    let (stem, extension) = split_extension(name);
    let stem = match tag_version(tag).and_then(|v| find_version(stem, v)) {
        Some((start, end)) => format!("{}*{}", &stem[..start], &stem[end..]),
        None => wildcard_dotted_numbers(stem),
    };
    format!("{stem}{extension}")
}

/// Guesses the pattern that picks the same file out of later uploads from
/// a file's display name, which unlike a file name has no extension: like
/// [`infer`], the file's `version` in the name becomes `*`, or failing that
/// any dotted numbers do, so `SkyUI 5.2 SE` is `SkyUI * SE`.
pub fn infer_title(title: &str, version: Option<&str>) -> String {
    match version.and_then(|v| find_version(title, tag_version(v)?)) {
        Some((start, end)) => format!("{}*{}", &title[..start], &title[end..]),
        None => wildcard_dotted_numbers(title),
    }
}

/// Where `version` is in `text`, as a byte range, ignoring ASCII case: its
/// last appearance that isn't part of a longer number, taking in any more
/// `.N` parts after it (`5.4.22` in `5.4.22.0` is all of `5.4.22.0`).
fn find_version(text: &str, version: &str) -> Option<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    let version = version.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let digit_at = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_digit);
    let mut found = None;
    for (start, _) in lower.match_indices(&version) {
        let mut end = start + version.len();
        while bytes.get(end) == Some(&b'.') && digit_at(end + 1) {
            end += 1;
            while digit_at(end) {
                end += 1;
            }
        }
        let alone = (start == 0 || !digit_at(start - 1)) && !digit_at(end);
        if alone {
            found = Some((start, end));
        }
    }
    found
}

/// `text` with each run of dotted numbers (`1.2`, `0.1.2`, but not a lone
/// `64`) replaced by `*`.
fn wildcard_dotted_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() && (i == 0 || !chars[i - 1].is_ascii_alphanumeric()) {
            // digits, then one or more `.digits`
            let mut end = i;
            let mut groups = 0;
            loop {
                let start = end;
                while end < chars.len() && chars[end].is_ascii_digit() {
                    end += 1;
                }
                if end == start {
                    break;
                }
                groups += 1;
                if end + 1 < chars.len() && chars[end] == '.' && chars[end + 1].is_ascii_digit() {
                    end += 1;
                } else {
                    break;
                }
            }
            if groups > 1 {
                out.push('*');
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Whether `name` fits `pattern`.
pub fn matches(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        // no `*` at all
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// `pattern` with its first `*` filled in with `version`, which is the name
/// a release's file most likely has when a pattern matches more than one.
pub fn fill(pattern: &str, version: &str) -> String {
    pattern.replacen('*', version, 1)
}
