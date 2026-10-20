use super::*;

#[test]
fn sorts_parse_from_their_names() {
    for sort in SortBy::ALL {
        assert_eq!(sort.as_str().parse::<SortBy>().unwrap(), sort);
    }
    let err = "popular".parse::<SortBy>().unwrap_err();
    assert!(err.to_string().contains("relevance, downloads"), "{err}");
}

#[test]
fn blank_text_is_no_text() {
    let query = |text: &str| SearchQuery {
        text: Some(text.to_owned()),
        ..Default::default()
    };
    assert_eq!(query("  skyui ").text(), Some("skyui"));
    assert_eq!(query("   ").text(), None);
    assert_eq!(SearchQuery::default().text(), None);
}
