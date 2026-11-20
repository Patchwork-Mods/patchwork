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

## Options

A theme can offer settings that show in **Settings → Appearance** (for the
themes in use) and in a game's settings (for its theme), so its look can be
changed without touching files. Each is an `[[option]]` table in the
`@theme` comment, after the paragraph:

```css
/*
 * @theme
 * name = "Hollow Knight: Silksong"
 * game = "hollow-knight-silksong"
 *
 * A paragraph saying what it looks like.
 *
 * [[option]]
 * id = "sparks"
 * label = "Sparks"
 * type = "choice"
 * var = "--sk-sparks"
 * choices = [
 *   { value = "moving", label = "Moving", css = "running" },
 *   { value = "still", label = "Still", css = "paused" },
 *   { value = "hidden", label = "Hidden", css = "paused" },
 * ]
 *
 * [[option]]
 * id = "glow"
 * label = "Glow"
 * description = "How bright the light behind everything is."
 * type = "range"
 * var = "--sk-glow"
 * min = 0
 * max = 100
 * default = 100
 */
```

An option sets the CSS variable `var` on `:root`, over what the theme sets
there, so the theme uses it with `var(--sk-glow)` and sets the default
itself too. The types:

| `type` | Keys | The variable gets |
| --- | --- | --- |
| `toggle` | `default` (true or false), `on`, `off` | `on` or `off` |
| `choice` | `choices` (`value`, `label`, optional `css`), `default` (a `value`; the first if left out) | the choice's `css`, or its `value` |
| `color` | `default` (`"#rrggbb"`) | the color |
| `range` | `min`, `max`, `default`, optional `step` and `unit` | the number with the unit after it |

A variable can't turn a rule on or off, so toggles and choices also set
`data-opt-<id>` on `:root`, to `on`/`off` or the choice's `value`:

```css
:root[data-opt-sparks="hidden"] body::before {
  display: none;
}
```

A range shows its number beside it, with the `unit` after it. Leave the
unit out to show just the number, and work it out in the CSS instead, like
`opacity: calc(var(--sk-glow) / 100)`. Each slider has `--pw-range` on it,
from 0 to 1, so a theme can draw the line before the knob differently from
the line after it (the Silksong theme does).

Ids are lower-case letters, digits and dashes. Only variables set on
`:root` itself can be changed this way: one the theme sets again on another
element keeps that value there. What's picked is kept in `<config>/gui.toml`
under `[theme.options.<theme id>]`, and `user.css` still goes on top.

## Motion

Anything that moves should stop under `:root[data-motion="reduced"]`,
which is set when **Reduce motion** is on in **Settings → Appearance** or
the system asks for less motion, and pause under `:root[data-inactive]`,
set while the window is in the background.

## Pictures and fonts

A theme that needs files of its own is a folder instead of one file:
`<id>/theme.css`, with the files beside it, which the CSS links to with
relative URLs:

```
themes/
  my-theme/
    theme.css
    fonts/cinzel.woff2
    img/frame-corner.svg
```

```css
@font-face {
  font-family: "Cinzel";
  src: url("fonts/cinzel.woff2") format("woff2");
}
#main {
  background: url("img/frame-corner.svg") top left no-repeat;
}
```

Only `url()` is followed (not `image-set()` with bare strings, nor
`@import`), and only inside the folder. Fonts (`.woff2`, `.woff`, `.ttf`,
`.otf`) are put into the CSS as it loads, so keep them small, and only use
fonts whose licence lets them be passed on, such as the SIL Open Font
Licence. Changing any file in the folder shows straight away, like the CSS.
Pictures from the web work too, as `url("https://...")`; fonts from the web
don't.

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
