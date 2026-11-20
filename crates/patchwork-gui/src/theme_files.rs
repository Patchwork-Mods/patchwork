//! Theme folders. A theme can be a folder, `<id>/theme.css`, with the
//! pictures and fonts it uses beside it. When it's loaded, each relative
//! `url()` in its CSS is pointed at the file it names. Fonts go into the CSS
//! itself as `data:` URLs, since WebKitGTK won't load fonts from another
//! scheme than the page's. Everything else is served by the [`SCHEME`]
//! protocol, at `<source>/<id>/<path>`: the source is `b` for a theme built
//! in from `registry/themes/`, or the index of the themes folder it's in.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use base64::Engine;
use tauri::http::{Response, StatusCode, header};

/// The protocol that serves themes' files.
pub const SCHEME: &str = "pw-theme";

/// Where the [`SCHEME`] protocol is, as the webview sees it.
#[cfg(windows)]
const BASE: &str = "http://pw-theme.localhost/";
#[cfg(not(windows))]
const BASE: &str = "pw-theme://localhost/";

/// The name of a theme folder's CSS file.
pub const CSS: &str = "theme.css";

/// Where a theme folder's files are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Folder {
    /// Built in, from `registry/themes/<id>/`.
    Builtin,
    /// The folder `dir`, in the `layer`th themes folder.
    Dir { layer: usize, dir: PathBuf },
}

impl Folder {
    fn source(&self) -> String {
        match self {
            Folder::Builtin => "b".to_owned(),
            Folder::Dir { layer, .. } => layer.to_string(),
        }
    }

    /// The file at `path` (as [`normalize`] gives it) in theme `id`'s folder.
    fn read(&self, id: &str, path: &str) -> Option<Cow<'static, [u8]>> {
        match self {
            Folder::Builtin => crate::themes::REGISTRY_FILES
                .iter()
                .find(|(i, p, _)| *i == id && *p == path)
                .map(|(_, _, bytes)| Cow::Borrowed(*bytes)),
            Folder::Dir { dir, .. } => {
                // A link inside the folder mustn't lead out of it.
                let root = dir.canonicalize().ok()?;
                let file = dir.join(path).canonicalize().ok()?;
                if !file.starts_with(&root) || !file.is_file() {
                    return None;
                }
                std::fs::read(file).ok().map(Cow::Owned)
            }
        }
    }
}

/// `css` from theme `id`'s folder, with its relative `url()`s pointed at
/// the folder's files.
pub fn link(css: &str, id: &str, folder: &Folder) -> String {
    rewrite_urls(css, &|url| {
        let end = url.find(['?', '#']).unwrap_or(url.len());
        let (path, suffix) = url.split_at(end);
        let path = normalize(path)?;
        if let Some(mime) = font_type(&path)
            && let Some(bytes) = folder.read(id, &path)
        {
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            return Some(format!("data:{mime};base64,{data}"));
        }
        let mut out = format!("{BASE}{}/{}/{}", folder.source(), encode(id), encode(&path));
        // A file that changes gets a new URL, so the webview doesn't keep
        // showing the old one.
        let version = match folder {
            Folder::Builtin => None,
            Folder::Dir { dir, .. } => modified(&dir.join(&path)),
        };
        match (version, suffix.strip_prefix('?')) {
            (Some(v), Some(query)) => out.push_str(&format!("?v={v}&{query}")),
            (Some(v), None) => out.push_str(&format!("?v={v}{suffix}")),
            (None, _) => out.push_str(suffix),
        }
        Some(out)
    })
}

/// When `path` last changed, in milliseconds.
fn modified(path: &Path) -> Option<u128> {
    let time = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(time.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis())
}

/// Answers a request for `path` (`/<source>/<id>/<file>`). `dirs` are the
/// themes folders, in the order they were loaded.
pub fn respond(dirs: &[PathBuf], path: &str) -> Response<Cow<'static, [u8]>> {
    let file = (|| {
        let (source, rest) = path.strip_prefix('/')?.split_once('/')?;
        let (id, file) = rest.split_once('/')?;
        let id = normalize(id).filter(|id| !id.contains('/'))?;
        let file = normalize(file)?;
        let folder = match source {
            "b" => Folder::Builtin,
            n => {
                let layer: usize = n.parse().ok()?;
                let dir = dirs.get(layer)?.join(&id);
                Folder::Dir { layer, dir }
            }
        };
        Some((folder.read(&id, &file)?, file))
    })();
    let response = Response::builder().header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    match file {
        Some((bytes, file)) => response
            .header(header::CONTENT_TYPE, content_type(&file))
            .body(bytes),
        None => response
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(&[][..])),
    }
    .expect("a valid response")
}

/// Calls `f` with each `url()` in `css`, and puts what it gives back in
/// its place, or leaves the `url()` alone if it gives back nothing. Only
/// relative URLs are passed: not `data:`, `https:` and the like, nor ones
/// starting with `/` or `#`. Comments and strings are skipped.
fn rewrite_urls(css: &str, f: &dyn Fn(&str) -> Option<String>) -> String {
    let bytes = css.as_bytes();
    let mut out = String::with_capacity(css.len());
    // Copied up to here.
    let mut done = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = css[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |e| i + 2 + e + 2);
            }
            quote @ (b'"' | b'\'') => i = string_end(bytes, i + 1, quote),
            b'u' | b'U'
                if css[i..]
                    .get(..4)
                    .is_some_and(|s| s.eq_ignore_ascii_case("url("))
                    && (i == 0 || !is_name_byte(bytes[i - 1])) =>
            {
                let Some((value, end)) = url_value(css, i + 4) else {
                    i += 4;
                    continue;
                };
                if is_relative(value)
                    && let Some(new) = f(value)
                {
                    out.push_str(&css[done..i]);
                    out.push_str("url(\"");
                    out.push_str(&new);
                    out.push_str("\")");
                    done = end;
                }
                i = end;
            }
            _ => i += 1,
        }
    }
    out.push_str(&css[done..]);
    out
}

/// Where the string that starts before `from` ends, after its closing quote.
fn string_end(bytes: &[u8], mut from: usize, quote: u8) -> usize {
    while from < bytes.len() {
        match bytes[from] {
            b'\\' => from += 2,
            b if b == quote => return from + 1,
            _ => from += 1,
        }
    }
    bytes.len()
}

/// The URL in a `url(` whose contents start at `from`, and where the
/// `url()` ends, after its `)`.
fn url_value(css: &str, from: usize) -> Option<(&str, usize)> {
    let bytes = css.as_bytes();
    let start = from + css[from..].len() - css[from..].trim_start().len();
    match bytes.get(start)? {
        &quote @ (b'"' | b'\'') => {
            let end = string_end(bytes, start + 1, quote);
            let value = css.get(start + 1..end.checked_sub(1)?)?;
            let close = end + css[end..].find(')')?;
            css[end..close]
                .trim()
                .is_empty()
                .then_some((value, close + 1))
        }
        _ => {
            let close = start + css[start..].find(')')?;
            Some((css[start..close].trim_end(), close + 1))
        }
    }
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Whether `url` is relative to the CSS it's in: no scheme, and not
/// starting with `/` or `#`.
fn is_relative(url: &str) -> bool {
    if url.is_empty() || url.starts_with(['/', '#', '\\']) {
        return false;
    }
    let before = url.find(['/', '?', '#']).unwrap_or(url.len());
    !url[..before].contains(':')
}

/// `path` decoded and with `.` and `..` worked out, or `None` if it leads
/// outside the folder or has something in it that can't be in a file name.
fn normalize(path: &str) -> Option<String> {
    let path = decode(path)?;
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ if part.contains(['\\', ':', '\0']) => return None,
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// `s` with its `%XX` escapes decoded.
fn decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

/// `s` with everything but letters, digits, `-._~` and `/` escaped.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// A font's media type, or `None` if `path` isn't a font.
fn font_type(path: &str) -> Option<&'static str> {
    Some(match extension(path).as_str() {
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => return None,
    })
}

fn content_type(path: &str) -> &'static str {
    font_type(path).unwrap_or(match extension(path).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "css" => "text/css",
        _ => "application/octet-stream",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(css: &str) -> String {
        rewrite_urls(css, &|url| Some(format!("<{url}>")))
    }

    #[test]
    fn rewrites_relative_urls() {
        assert_eq!(rewrite("a{b:url(x.png)}"), "a{b:url(\"<x.png>\")}");
        assert_eq!(rewrite("a{b:URL( 'x y.png' )}"), "a{b:url(\"<x y.png>\")}");
        assert_eq!(
            rewrite("a{b:url(\"a.svg#i\"), url(b.png)}"),
            "a{b:url(\"<a.svg#i>\"), url(\"<b.png>\")}"
        );
        assert_eq!(rewrite("a{b:url( c.png )}"), "a{b:url(\"<c.png>\")}");
    }

    #[test]
    fn leaves_other_urls_alone() {
        for css in [
            "a{b:url(https://x/y.png)}",
            "a{b:url(data:image/png;base64,AA==)}",
            "a{b:url(/x.png)}",
            "a{b:url(#grad)}",
            "/* url(x.png) */",
            "a{content:\"url(x.png)\"}",
            "a{b:myurl(x.png)}",
            "a{b:url(x.png}",
        ] {
            assert_eq!(rewrite(css), css);
        }
    }

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize("./img/../a%20b.png").as_deref(), Some("a b.png"));
        assert_eq!(normalize("../x.png"), None);
        assert_eq!(normalize("a/%2e%2e/%2e%2e/x"), None);
        assert_eq!(normalize("c:/x"), None);
        assert_eq!(normalize("a\\b"), None);
        assert_eq!(encode("a b/ü.png"), "a%20b/%C3%BC.png");
    }

    #[test]
    fn links_a_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("mine");
        std::fs::create_dir_all(dir.join("img")).unwrap();
        std::fs::write(dir.join("img/bg.png"), b"png").unwrap();
        std::fs::write(dir.join("font.woff2"), b"font").unwrap();
        std::fs::write(tmp.path().join("secret.txt"), b"no").unwrap();
        let folder = Folder::Dir {
            layer: 2,
            dir: dir.clone(),
        };
        let css = link(
            "a{background:url(img/bg.png?x=1)} @font-face{src:url('font.woff2')}",
            "mine",
            &folder,
        );
        let v = modified(&dir.join("img/bg.png")).unwrap();
        assert_eq!(
            css,
            format!(
                "a{{background:url(\"{BASE}2/mine/img/bg.png?v={v}&x=1\")}} \
                 @font-face{{src:url(\"data:font/woff2;base64,Zm9udA==\")}}"
            )
        );

        let dirs = [PathBuf::new(), PathBuf::new(), tmp.path().to_owned()];
        let ok = respond(&dirs, "/2/mine/img/bg.png");
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(ok.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(&ok.body()[..], b"png");
        for bad in [
            "/2/mine/../secret.txt",
            "/2/mine/%2e%2e/secret.txt",
            "/2/../secret.txt",
            "/2/mine/missing.png",
            "/9/mine/img/bg.png",
            "/b/mine/img/bg.png",
        ] {
            assert_eq!(respond(&dirs, bad).status(), StatusCode::NOT_FOUND, "{bad}");
        }
    }
}
