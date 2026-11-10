# Example themes

Themes are plain CSS loaded after Patchwork's built-in look, so each one
only sets the `--pw-*` variables and the few rules it changes. A comment
starting with `@theme` says what it is:

```css
/*
 * @theme
 * name = "Risk of Rain 2"
 * game = "risk-of-rain-2"   (leave out for a theme for every game)
 * modes = ["dark"]          (the modes it has looks for)
 * layer = false             (true: go on top of the picked theme, not replace it)
 *
 * A paragraph saying what it looks like.
 */
```

Games' own themes come with the registry, in
[`registry/themes/`](../../registry/themes): `risk-of-rain-2.css` is a
full one to start from (cut-corner HUD panels, item rarity colours on mod
cards, health-bar switches). Copy one into `<config>/themes/` to change it:
a file there with the same name takes its place. Themes for every game,
such as Sorbet, Phosphor and Blueprint, come with Patchwork (in
`crates/patchwork-gui/themes/`) and are picked in **Settings →
Appearance**, which lists your own themes for every game beside them, or
in `<config>/gui.toml`:

```toml
[theme]
theme = "sorbet"
game_themes = "on"   # or "off"
```

For a look in both modes, put the dark one under `:root[data-mode="dark"]`
(as `sorbet.css` does) rather than a `prefers-color-scheme` media query, so
it follows the mode picked in **Settings → Appearance**. Themes load in
cascade layers after the built-in look, so their selectors never need to be
more specific than the built-in ones.

A file in `<config>/themes/` with the same name as a built-in theme takes
its place. `user.css` loads last, on top of whichever theme is in use.
Patchwork watches `<config>/themes/` (and the `themes/` folders of
registries, including `$PATCHWORK_REGISTRY_PATH`), so changes show as soon
as a file is saved.
