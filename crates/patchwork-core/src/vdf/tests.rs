use super::*;

#[test]
fn reads_nested_blocks() {
    let text = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/games\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"489830\"\t\t\"123\"\n\t\t}\n\t}\n}\n";
    let root = parse(text).unwrap();
    let library = root.get("LibraryFolders").unwrap().get("0").unwrap();
    assert_eq!(library.get_str("path"), Some("/games"));
    assert_eq!(library.get("apps").unwrap().get_str("489830"), Some("123"));
}

#[test]
fn unescapes_strings() {
    let root = parse(r#""path" "D:\\SteamLibrary" "q" "say \"hi\"""#).unwrap();
    assert_eq!(root.get_str("path"), Some(r"D:\SteamLibrary"));
    assert_eq!(root.get_str("q"), Some(r#"say "hi""#));
}

#[test]
fn skips_comments_and_conditionals() {
    let text = "// a comment\n\"a\" \"1\" [$WIN32]\nb { c 2 } // trailing\n";
    let root = parse(text).unwrap();
    assert_eq!(root.get_str("a"), Some("1"));
    assert_eq!(root.get("b").unwrap().get_str("c"), Some("2"));
}

#[test]
fn rejects_what_isnt_keyvalues() {
    assert_eq!(parse("\"a\" { \"b\" \"c\""), None);
    assert_eq!(parse("\"a\""), None);
    assert_eq!(parse("}"), None);
    assert_eq!(parse("\"unterminated"), None);
}
