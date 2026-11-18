#!/usr/bin/env bash
# Writes target/licenses/, the license notices that ship with release builds:
# THIRD-PARTY-LICENSES.txt (every Rust dependency's license, from cargo-about
# with about.toml and about.hbs here) and the vendored quickentity-rs's
# LGPL-3.0 and the GPL-3.0 it builds on. dist puts the folder in the CLI
# archives (include in dist-workspace.toml) and Tauri in the app bundles
# (resources in tauri.release.conf.json); our own LICENSE goes beside it.
#
# Needs cargo-about (`nix shell nixpkgs#cargo-about`). Crates without a
# license are left out with a warning: cargo deny is what fails a release
# over those.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/target/licenses"
rm -rf "$out"
mkdir -p "$out"
cd "$root"
cargo about generate --locked \
    --config packaging/about.toml \
    --output-file "$out/THIRD-PARTY-LICENSES.txt" \
    packaging/about.hbs
cp vendor/quickentity-rs/COPYING.LESSER "$out/LGPL-3.0.txt"
cp vendor/quickentity-rs/COPYING "$out/GPL-3.0.txt"
