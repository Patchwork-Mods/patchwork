//! Registry entries compiled into the binary, so patchwork works out of the
//! box. Registries and user files can override any of these.

macro_rules! entry {
    ($kind:literal, $id:literal) => {
        (
            $kind,
            $id,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../registry/",
                $kind,
                "/",
                $id,
                ".toml"
            )),
        )
    };
}

/// `(kind, id, contents)`
pub const ENTRIES: &[(&str, &str, &str)] = &[
    entry!("games", "lethal-company"),
    entry!("games", "risk-of-rain-2"),
    entry!("rulesets", "bepinex"),
];
