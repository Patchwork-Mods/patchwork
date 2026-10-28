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

#[test]
fn file_names_from_urls_are_decoded() {
    let name = |url: &str| {
        ArtifactLocation::Url {
            url: url.to_owned(),
        }
        .file_name()
    };
    assert_eq!(
        name("https://cdn.modrinth.com/data/x/sodium-fabric-0.8.13%2Bmc1.21.1.jar?a=b"),
        "sodium-fabric-0.8.13+mc1.21.1.jar"
    );
    assert_eq!(name("https://example.com/My%20Mod.zip"), "My Mod.zip");
    // left alone when decoding wouldn't make a file name
    assert_eq!(name("https://example.com/a%2Fb.zip"), "a%2Fb.zip");
    assert_eq!(name("https://example.com/100%.zip"), "100%.zip");
    assert_eq!(name("https://example.com/%FF.zip"), "%FF.zip");
}
