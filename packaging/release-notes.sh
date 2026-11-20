#!/usr/bin/env bash
# Writes a GitHub release's body to stdout. release.yml's announce job runs it
# in place of dist's own body, which is all about installing the CLI: this
# puts a table of the app's downloads first, then the release's CHANGELOG.md
# section, then dist's CLI install instructions folded away under <details>.
#
#   release-notes.sh <dir with the release's files> <tag> <owner/repo>
#
# with dist's manifest (host's output) in $MANIFEST, for its changelog and
# its body.
set -euo pipefail
dir=$1 tag=$2 repo=$3
base="https://github.com/$repo/releases/download/$tag"

changelog=$(jq -r '.announcement_changelog // ""' <<< "$MANIFEST")
body=$(jq -r '.announcement_github_body // ""' <<< "$MANIFEST")
# dist's body is the changelog followed by its CLI sections, which start at
# "## Install patchwork-cli <version>".
if [[ $body == "$changelog"* ]]; then
    cli=${body#"$changelog"}
elif [[ $body == *"## Install "* ]]; then
    cli="## Install ${body#*"## Install "}"
else
    cli=$body
fi

# link <label> <glob>: a markdown link to the one file matching <glob>, or
# nothing if there's none.
link() {
    local label=$1 file
    shift
    for file in "$dir"/$1; do
        [[ -f $file ]] || continue
        file=$(basename "$file")
        printf '[%s](%s/%s)' "$label" "$base" "$file"
        return
    done
}

# row <platform> <link>...: a table row of the links that exist, if any do.
row() {
    local platform=$1 links=() l
    shift
    for l in "$@"; do [[ -n $l ]] && links+=("$l"); done
    ((${#links[@]})) || return 0
    local IFS=$'\x1f'
    local joined="${links[*]}"
    printf '| %s | %s |\n' "$platform" "${joined//$'\x1f'/ · }"
}

rows=$(
    row "Windows" "$(link "Installer (.exe)" '*-setup.exe')"
    row "macOS (Apple Silicon)" "$(link "Disk image (.dmg)" '*.dmg')"
    row "Linux (x86_64)" \
        "$(link "AppImage" '*_amd64.AppImage')" \
        "$(link ".deb" '*_amd64.deb')" \
        "$(link ".rpm" '*.x86_64.rpm')"
    row "Linux (ARM64)" \
        "$(link "AppImage" '*_aarch64.AppImage')" \
        "$(link ".deb" '*_arm64.deb')" \
        "$(link ".rpm" '*.aarch64.rpm')"
)

if [[ -n $rows ]]; then
    printf '## Download Patchwork\n\n| Platform | Download |\n|---|---|\n%s\n\n' "$rows"
fi
if [[ -n $changelog ]]; then
    printf "## What's new\n\n%s\n\n" "$changelog"
fi
if [[ -n ${cli//[[:space:]]/} ]]; then
    # Blank lines around the markdown, or GitHub shows it as plain text.
    printf '<details>\n<summary>Command-line only (patchwork-cli)</summary>\n\n%s\n\n</details>\n' "$cli"
fi
