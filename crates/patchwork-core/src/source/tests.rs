use super::*;

#[test]
fn orders_dotted_versions() {
    assert_eq!(
        compare_dotted("5.4.2100", "5.4.21"),
        Some(Ordering::Greater)
    );
    assert_eq!(compare_dotted("1.2", "1.2.0"), Some(Ordering::Equal));
    assert_eq!(compare_dotted("1.9", "1.10"), Some(Ordering::Less));
    assert_eq!(compare_dotted("5.2SE", "5.1"), None);
}

#[test]
fn version_reqs_round_trip_as_strings() {
    for (text, req) in [
        ("*", VersionReq::Any),
        (">=5.4.2100", VersionReq::AtLeast("5.4.2100".to_owned())),
        ("=1.0", VersionReq::Exact("1.0".to_owned())),
    ] {
        assert_eq!(text.parse::<VersionReq>().unwrap(), req);
        assert_eq!(req.to_string(), text);
    }
    assert!("1.0".parse::<VersionReq>().is_err());
    assert!(">=".parse::<VersionReq>().is_err());
}

#[test]
fn version_reqs_match() {
    let at_least = VersionReq::AtLeast("1.2".to_owned());
    assert_eq!(at_least.matches("1.10", compare_dotted), Some(true));
    assert_eq!(at_least.matches("1.1.9", compare_dotted), Some(false));
    assert_eq!(at_least.matches("beta", compare_dotted), None);
    let exact = VersionReq::Exact("1.2".to_owned());
    assert_eq!(exact.matches("1.2.0", compare_dotted), Some(true));
    assert_eq!(exact.matches("1.3", compare_dotted), Some(false));
    assert_eq!(VersionReq::Any.matches("beta", compare_dotted), Some(true));
}
