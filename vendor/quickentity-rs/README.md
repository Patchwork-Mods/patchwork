# quickentity-rs (vendored)

[QuickEntity](https://github.com/atampy25/quickentity-rs) converts HITMAN
entities between the game's TEMP/TBLU form and QuickEntity JSON, and applies
QuickEntity patches. The SMF deployer uses it for `.entity.json` and
`.entity.patch.json` files.

Upstream needs nightly Rust, so it's vendored here at commit `2e7a3563`
(library only; the CLI and the `rune`/`tracing` integrations are dropped)
with these changes:

- `src/lib.rs`: `Iterator::try_find` (unstable) is replaced by an equivalent
  `find_ok` extension method.
- `auto_context/src/lib.rs`: `proc_macro::quote!` (unstable) is replaced by
  building the tokens by hand, and `Span::start().line()` by `Span::line()`.

Licensed under the LGPL-3.0; see `COPYING` and `COPYING.LESSER`.
