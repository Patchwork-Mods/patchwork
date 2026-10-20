use super::RelPath;

#[test]
fn normalizes() {
    assert_eq!(RelPath::new("a\\b/./c//").unwrap().as_str(), "a/b/c");
}

#[test]
fn rejects_escapes() {
    for bad in [
        "",
        "/etc/passwd",
        "a/../../b",
        "..",
        "C:/Windows",
        "\\\\server\\x",
    ] {
        assert!(RelPath::new(bad).is_err(), "{bad} should be rejected");
    }
}

#[test]
fn parent_and_name() {
    let p = RelPath::new("BepInEx/plugins/x.dll").unwrap();
    assert_eq!(p.file_name(), "x.dll");
    assert_eq!(p.parent().unwrap().as_str(), "BepInEx/plugins");
    assert_eq!(RelPath::new("x").unwrap().parent(), None);
}
