use std::io::{Read, Write};
use std::net::TcpListener;

use super::*;

/// The hosts that come with patchwork.
fn builtin() -> Links {
    let hosts: Vec<_> = crate::builtin::ENTRIES
        .iter()
        .filter(|(kind, _, _)| *kind == KIND)
        .map(|(_, id, text)| (*id, *text))
        .collect();
    assert!(!hosts.is_empty());
    Links::parse(&hosts).unwrap()
}

/// Answers requests by path, for as long as the test runs: `(path,
/// headers, body)`, where `headers` is the status line's rest and any
/// headers. Gives the server's base URL.
fn serve(routes: Vec<(&'static str, &'static str, String)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let routes: Vec<_> = routes
        .into_iter()
        .map(|(path, head, body)| (path, head, body.replace("{base}", &base)))
        .collect();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = [0; 2048];
            let n = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]);
            let path = request.split_whitespace().nth(1).unwrap_or_default();
            let (head, body) = routes
                .iter()
                .find(|(p, _, _)| *p == path)
                .map_or(("404 Not Found", ""), |(_, h, b)| (*h, b.as_str()));
            let _ = write!(
                stream,
                "HTTP/1.1 {head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    base
}

fn host(text: &str, base: &str) -> Links {
    let text = text.replace("{base}", &regex::escape(base));
    Links::parse(&[("test", &text)]).unwrap()
}

#[test]
fn builtin_hosts_recognize_their_links() {
    let links = builtin();
    let cases = [
        (
            "https://www.dropbox.com/scl/fi/abc123/Cool%20Mod.ftl?rlkey=xyz&dl=0",
            "dropbox",
            "https://www.dropbox.com/scl/fi/abc123/Cool%20Mod.ftl?rlkey=xyz&dl=0",
            Some("Cool Mod.ftl"),
        ),
        (
            "https://www.dropbox.com/s/abc123/mod.zip?dl=0",
            "dropbox",
            "https://www.dropbox.com/s/abc123/mod.zip?dl=0",
            Some("mod.zip"),
        ),
        (
            "https://drive.google.com/file/d/1AbC-d_E/view?usp=sharing",
            "google-drive",
            "https://drive.google.com/file/d/1AbC-d_E/view",
            None,
        ),
        (
            "https://drive.google.com/open?id=1AbC-d_E",
            "google-drive",
            "https://drive.google.com/file/d/1AbC-d_E/view",
            None,
        ),
        (
            "https://drive.google.com/uc?export=download&id=1AbC-d_E",
            "google-drive",
            "https://drive.google.com/file/d/1AbC-d_E/view",
            None,
        ),
        (
            "https://www.mediafire.com/file/k3y/Some_Mod.ftl/file",
            "mediafire",
            "https://www.mediafire.com/file/k3y/Some_Mod.ftl/file",
            Some("Some_Mod.ftl"),
        ),
        (
            "http://mediafire.com/download/k3y",
            "mediafire",
            "https://www.mediafire.com/?k3y",
            None,
        ),
        (
            "https://www.mediafire.com/?k3y",
            "mediafire",
            "https://www.mediafire.com/?k3y",
            None,
        ),
        (
            "https://moddb.com/mods/ftl-multiverse/downloads/ftl-multiverse-5-3?x=1",
            "moddb",
            "https://www.moddb.com/mods/ftl-multiverse/downloads/ftl-multiverse-5-3",
            None,
        ),
        (
            "https://www.indiedb.com/games/g/downloads/demo",
            "moddb",
            "https://www.indiedb.com/games/g/downloads/demo",
            None,
        ),
    ];
    for (url, host, link, file) in cases {
        let found = links.find(url).unwrap_or_else(|| panic!("{url}"));
        assert_eq!(found.host_id(), host, "{url}");
        assert_eq!(found.link, link, "{url}");
        assert_eq!(found.file.as_deref(), file, "{url}");
    }
    assert!(
        links
            .find("https://www.moddb.com/mods/x/downloads")
            .is_none()
    );
    assert!(links.find("https://example.com/mod.zip").is_none());
    assert!(
        links
            .find("https://drive.google.com/drive/folders/abc")
            .is_none()
    );
    assert!(links.find("https://www.moddb.com/mods/x").is_none());

    let moddb = links
        .find("https://www.moddb.com/mods/x/downloads/y")
        .unwrap();
    assert!(moddb.browser_only());
    assert_eq!(moddb.format(), Some(ArtifactFormat::Archive));
}

#[tokio::test]
async fn builtin_hosts_without_steps_rewrite_links() {
    let links = builtin();
    let dropbox = links
        .find("https://www.dropbox.com/scl/fi/abc/mod.zip?rlkey=xyz&dl=0")
        .unwrap();
    assert_eq!(
        dropbox.direct().await.unwrap().url,
        "https://www.dropbox.com/scl/fi/abc/mod.zip?rlkey=xyz&dl=1"
    );
    let drive = links.find("https://drive.google.com/open?id=1Ab").unwrap();
    assert_eq!(
        drive.direct().await.unwrap().url,
        "https://drive.usercontent.google.com/download?id=1Ab&export=download&confirm=t"
    );
}

#[test]
fn templates_take_the_first_that_can_be_filled() {
    let vars: BTreeMap<_, _> = [("a", "1"), ("empty", "")]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    assert_eq!(expand("x/${a}/y", &vars).as_deref(), Some("x/1/y"));
    assert_eq!(expand("plain", &vars).as_deref(), Some("plain"));
    assert_eq!(expand("${b}", &vars), None);
    assert_eq!(expand("${empty}", &vars), None);
    assert_eq!(expand("${a", &vars), None);
    let templates = Templates(vec!["${b}".to_owned(), "${a}!".to_owned()]);
    assert_eq!(templates.expand(&vars).as_deref(), Some("1!"));
}

#[test]
fn bad_hosts_are_problems() {
    let mut registry = Registry::default();
    registry.add_data(KIND, "ok", "name = 'Ok'\nlinks = '^https://ok/'");
    registry.add_data(KIND, "no-links", "name = 'No'\nlinks = []");
    registry.add_data(KIND, "bad-regex", "name = 'Bad'\nlinks = '('");
    registry.add_data(KIND, "typo", "name = 'T'\nlinks = 'x'\nbrowsr = true");
    registry.add_data(
        KIND,
        "empty-find",
        "name = 'E'\nlinks = 'x'\n[[steps]]\nfind = []",
    );
    let (links, problems) = Links::from_registry(&registry);
    assert_eq!(links.hosts.len(), 1);
    assert_eq!(problems.len(), 4, "{problems:?}");
}

#[tokio::test]
async fn steps_follow_pages_to_the_file() {
    let base = serve(vec![
        (
            "/file/abc",
            "200 OK",
            r#"<h1>Mod &amp; more.zip</h1><a href="/start/1?a=1&amp;b=2">Download</a>"#.to_owned(),
        ),
        (
            "/start/1?a=1&b=2",
            "200 OK",
            r#"<a data-x="aHR0cHM6Ly9jZG4uZXhhbXBsZS9maWxlLnppcA==">mirror</a>"#.to_owned(),
        ),
    ]);
    let links = host(
        r#"
        name = "Test"
        links = '^{base}/f/(?<key>\w+)'
        link = "{page}/file/${key}"

        [[steps]]
        find = ['nothing here (.*)', 'href="(?<url>/start/[^"]+)"']
        name = '<h1>(?<file>[^<]+)</h1>'

        [[steps]]
        find = 'data-x="(?<b64>[^"]+)"'
        "#
        .replace("{page}", &base)
        .as_str(),
        &base,
    );
    let found = links.find(&format!("{base}/f/abc")).unwrap();
    assert_eq!(found.link, format!("{base}/file/abc"));
    let direct = found.direct().await.unwrap();
    assert_eq!(direct.url, "https://cdn.example/file.zip");
    assert_eq!(direct.file.as_deref(), Some("Mod & more.zip"));
    assert_eq!(found.file_name().await.unwrap(), "Mod & more.zip");
}

#[tokio::test]
async fn missing_downloads_and_refusals_are_errors() {
    let base = serve(vec![
        ("/changed", "200 OK", "<p>a new layout</p>".to_owned()),
        ("/blocked", "403 Forbidden", String::new()),
    ]);
    let links = host(
        r#"
        name = "Test"
        links = '^{base}/'
        [[steps]]
        find = 'href="([^"]+)"'
        "#,
        &base,
    );
    let err = links
        .find(&format!("{base}/changed"))
        .unwrap()
        .direct()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("may have changed"), "{err}");
    assert!(!refused(&err));
    let err = links
        .find(&format!("{base}/blocked"))
        .unwrap()
        .direct()
        .await
        .unwrap_err();
    assert!(refused(&err), "{err}");
}

#[tokio::test]
async fn file_names_come_from_the_download() {
    let base = serve(vec![
        (
            "/named",
            "200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; \
             filename=\"plain.zip\"; filename*=UTF-8''Caf%C3%A9.zip",
            "zip".to_owned(),
        ),
        (
            "/dir/unnamed.7z",
            "200 OK\r\nContent-Type: application/octet-stream",
            "7z".to_owned(),
        ),
        (
            "/page",
            "200 OK\r\nContent-Type: text/html; charset=utf-8",
            "<html>quota</html>".to_owned(),
        ),
    ]);
    let links = host("name = 'Test'\nlinks = '^{base}/'", &base);
    let name = |path: &str| {
        let found = links.find(&format!("{base}{path}")).unwrap();
        async move { found.file_name().await }
    };
    assert_eq!(name("/named").await.unwrap(), "Café.zip");
    assert_eq!(name("/dir/unnamed.7z").await.unwrap(), "unnamed.7z");
    let err = name("/page").await.unwrap_err();
    assert!(err.to_string().contains("web page"), "{err}");
}

#[test]
fn reads_content_disposition() {
    assert_eq!(
        disposition_file_name(r#"attachment; filename="a b.zip""#).as_deref(),
        Some("a b.zip")
    );
    assert_eq!(
        disposition_file_name("attachment; filename=x.ftl").as_deref(),
        Some("x.ftl")
    );
    assert_eq!(
        disposition_file_name(r#"attachment; filename="../../evil.zip""#).as_deref(),
        Some("evil.zip")
    );
    assert_eq!(disposition_file_name("inline"), None);
}

#[test]
fn unescapes_html() {
    assert_eq!(unescape_html("a&amp;b&#38;c&#x26;d"), "a&b&c&d");
    assert_eq!(unescape_html("AT&T &unknown; &"), "AT&T &unknown; &");
}

#[test]
fn sets_query_parameters() {
    let query = BTreeMap::from([("dl".to_owned(), "1".to_owned())]);
    assert_eq!(
        with_query("https://x/a?dl=0&k=v", &query).unwrap(),
        "https://x/a?k=v&dl=1"
    );
    assert_eq!(
        with_query("https://x/a", &query).unwrap(),
        "https://x/a?dl=1"
    );
}
