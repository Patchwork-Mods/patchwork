use super::*;

#[test]
fn names_are_sanitized() {
    assert_eq!(sanitize_name("My Cool Pack!"), "My-Cool-Pack");
    assert_eq!(sanitize_name("  a  /  b "), "a-b");
    assert_eq!(sanitize_name("ok_name-1.0"), "ok_name-1.0");
    assert_eq!(sanitize_name("!!!"), "imported");
}
