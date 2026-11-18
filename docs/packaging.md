# Packaging patchwork

Notes for anyone packaging patchwork for a package manager or distro.

## Updates

Our own installers (the Windows installer, the macOS app, the AppImage,
.deb and .rpm) update themselves from GitHub releases, and so does the CLI
installed with our shell/PowerShell installer. A package manager's copy
shouldn't, so tell patchwork who looks after it. Then:

- Check for updates (and `patchwork self-update`) still says when there's a
  newer release, but says to update it with your package manager, showing
  your update command if you give one, instead of offering to install or
  download it.
- The app doesn't check for new releases by itself when it opens, since your
  package usually arrives later than our release. You can turn that back on
  with `check-automatically`.

There are two ways to say it, and the first wins.

### Building from source

Set these in the build's environment (they're read at compile time):

```sh
PATCHWORK_PACKAGE_MANAGER="Homebrew"                 # "Patchwork was installed with Homebrew"
PATCHWORK_UPDATE_COMMAND="brew upgrade patchwork"    # optional
```

### Repackaging our builds

If you ship our binaries (an AUR `-bin` package, Scoop, ...), you can't
rebuild them. Bundles still say they came from our installer, so without
this they try to update themselves. Instead, add a `packaging.toml` either
beside the executable or in `<prefix>/share/patchwork/` (for an executable in
`<prefix>/bin/`). If the executable is reached through a link, patchwork also
looks where the link points.

```toml
# Who updates this copy, as in "Patchwork was installed with ...".
manager = "the AUR"
# Optional: the command that updates it, shown with a copy button.
update-command = "yay -Syu patchwork-bin"
# Optional: let the app check for new releases when it opens (default false).
check-automatically = false
```

## Paths that change on update

The background daemon's login entry (systemd user unit, autostart entry,
LaunchAgent or `Run` key) runs patchwork by the path it was started with, so
a link like `/opt/homebrew/bin/patchwork` or `~/.nix-profile/bin/patchwork`
survives updates. If the entry still names a copy that has since been
removed, the next run of the app or CLI points it at itself.

## Licenses

patchwork is MIT, but builds with the Hitman extension (the `smf` feature,
on by default) statically link the vendored quickentity-rs, which is
LGPL-3.0-only. So a default build's license is `MIT AND LGPL-3.0-only`.

Ship these with the binaries:

- `LICENSE` (patchwork's own)
- `target/licenses/`, written by `packaging/licenses.sh` (needs
  [cargo-about](https://github.com/EmbarkStudios/cargo-about)):
  `THIRD-PARTY-LICENSES.txt` with every Rust dependency's license, and the
  LGPL-3.0 and GPL-3.0 texts.

Our release archives and app bundles already contain all of them, in a
`licenses` folder.

## Nix

The flake has the app and CLI as `packages.default`
(`packaging/nix/package.nix`): `nix run github:patchwork-mods/patchwork`.
It builds with `PATCHWORK_PACKAGE_MANAGER=Nix`.
