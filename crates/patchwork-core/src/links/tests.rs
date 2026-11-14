use super::*;
use crate::ext::testing::serve;

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
    registry.add_data(
        KIND,
        "folder-groups",
        "name = 'F'\nlinks = 'x'\n[folder]\nlinks = 'y'\nentries = '(?<link>a)'",
    );
    let (links, problems) = Links::from_registry(&registry);
    assert_eq!(links.hosts.len(), 1);
    assert_eq!(problems.len(), 5, "{problems:?}");
}

#[test]
fn builtin_hosts_recognize_folder_links() {
    let links = builtin();
    for url in [
        "https://drive.google.com/drive/folders/1EYS-b_x?usp=sharing",
        "https://drive.google.com/drive/u/0/folders/1EYS-b_x",
        "https://drive.google.com/embeddedfolderview?id=1EYS-b_x#list",
    ] {
        let folder = links.folder(url).unwrap_or_else(|| panic!("{url}"));
        assert_eq!(
            folder.link,
            "https://drive.google.com/drive/folders/1EYS-b_x"
        );
        assert_eq!(folder.host_name(), "Google Drive");
    }
    assert!(
        links
            .folder("https://drive.google.com/file/d/1AbC/view")
            .is_none()
    );
    assert!(links.folder("https://example.com/folders/x").is_none());
}

/// Two entries as Google Drive's embedded folder view had them in 2026.
#[test]
fn builtin_drive_folder_entries_match_its_page() {
    let links = builtin();
    let drive = links.hosts.iter().find(|h| h.id == "google-drive").unwrap();
    let page = r#"<div class="flip-entries"><div class="flip-entry" id="entry-1ig" tabindex="0" role="link"><div class="flip-entry-info"><a href="https://drive.google.com/drive/folders/1ig" target="_blank"><div class="flip-entry-visual"><div class="flip-entry-visual-card"><div class="flip-entry-icon"><div aria-label="Folder" class="icon-color-1 drive-sprite-folder-grid-shared-icon"></div></div></div></div><div class="flip-entry-list-icon"><div aria-label="Folder" class="icon-color-1 drive-sprite-folder-list-shared-icon"></div></div><div class="flip-entry-title">Outdated Tech trees</div></a></div><div class="flip-entry-last-modified"><div>Aug 19</div></div></div><div class="flip-entry" id="entry-14ck" tabindex="0" role="link"><div class="flip-entry-info"><a href="https://drive.google.com/file/d/14ck/view?usp=drive_web" target="_blank"><div class="flip-entry-visual"><div class="flip-entry-visual-card"><div class="flip-entry-icon"><img src="https://drive-thirdparty.googleusercontent.com/128/type/application/x-zip-compressed" alt="Compressed Archive"/></div></div></div><div class="flip-entry-list-icon"><img src="https://drive-thirdparty.googleusercontent.com/16/type/application/x-zip-compressed" alt=""/></div><div class="flip-entry-title">Armory 1.1.zip</div></a></div><div class="flip-entry-last-modified"><div>8/12/23</div></div></div></div>"#;
    let found: Vec<_> = drive
        .folder
        .as_ref()
        .unwrap()
        .entries
        .captures_iter(page)
        .map(|c| (c["link"].to_owned(), c["name"].to_owned()))
        .collect();
    assert_eq!(
        found,
        [
            (
                "https://drive.google.com/drive/folders/1ig".to_owned(),
                "Outdated Tech trees".to_owned()
            ),
            (
                "https://drive.google.com/file/d/14ck/view?usp=drive_web".to_owned(),
                "Armory 1.1.zip".to_owned()
            ),
        ]
    );
}

#[tokio::test]
async fn folders_list_their_files_and_folders() {
    let base = serve(vec![
        (
            "/list?id=top",
            "200 OK",
            r#"
            <li><a href="/folder/sub">Old &amp; outdated</a></li>
            <li size="1234"><a href="{base}/f/abc?usp=x">Mod 1.0.zip</a></li>
            <li><a href="https://elsewhere.example/doc">A doc</a></li>
            <li><a href="/f/def">readme.pdf</a></li>
            "#
            .to_owned(),
        ),
        ("/list?id=empty", "200 OK", "<p>Sign in</p>".to_owned()),
    ]);
    // the base goes in regexes escaped and in templates as is
    let links = Links::parse(&[(
        "test",
        &r#"
        name = "Test"
        links = '^BASE_RE/f/(?<key>\w+)'
        link = "BASE/f/${key}"
        [folder]
        links = '^BASE_RE/folder/(?<id>\w+)'
        link = "BASE/folder/${id}"
        fetch = "BASE/list?id=${id}"
        entries = '<li(?: size="(?<size>\d+)")?><a href="(?<link>[^"]+)">(?<name>[^<]*)</a>'
        "#
        .replace("BASE_RE", &regex::escape(&base))
        .replace("BASE", &base),
    )])
    .unwrap();

    assert!(links.find(&format!("{base}/folder/top")).is_none());
    let folder = links.folder(&format!("{base}/folder/top?x=1")).unwrap();
    assert_eq!(folder.link, format!("{base}/folder/top"));
    let listed = folder.list().await.unwrap();
    let entry = |name: &str, link: String, folder, size| FolderEntry {
        name: name.to_owned(),
        link,
        folder,
        size,
    };
    assert_eq!(
        listed.entries,
        [
            entry("Old & outdated", format!("{base}/folder/sub"), true, None),
            entry("Mod 1.0.zip", format!("{base}/f/abc"), false, Some(1234)),
            entry("readme.pdf", format!("{base}/f/def"), false, None),
        ]
    );

    let err = links
        .folder(&format!("{base}/folder/empty"))
        .unwrap()
        .list()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("private"), "{err}");
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
