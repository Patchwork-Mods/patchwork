# patchwork: the app (patchwork-gui) and the CLI (patchwork), from this
# checkout. The flake's packages.default; `nix build` / `nix run`.
#
# Written to be close to what a nixpkgs package would be: everything comes
# from nixpkgs (not the dev shell's fenix toolchain), and git dependencies
# are fetched by their locked revisions.
{
  lib,
  stdenv,
  rustPlatform,
  pkg-config,
  wrapGAppsHook3,
  copyDesktopItems,
  makeDesktopItem,
  openssl,
  webkitgtk_4_1,
  gtk3,
  libsoup_3,
  glib-networking,
  librsvg,
}:

let
  root = ../..;
  fs = lib.fileset;

  # glacier-bin1-h3 reads ../CustomTypes.json and ../../variant_impl.rs, from
  # the root of its repository, which isn't there when Nix vendors each crate
  # on its own. So those files are put beside the vendored crates, from the
  # locked revision.
  glacierBin1 =
    let
      lock = lib.importTOML (root + "/Cargo.lock");
      h3 = lib.findFirst (
        p: p.name == "glacier-bin1-h3"
      ) (throw "no glacier-bin1-h3 in Cargo.lock") lock.package;
      # git+https://github.com/atampy25/glacier-bin1?rev=<short>#<full>
      source = builtins.match "git\\+([^?]+)\\?.*#(.*)" h3.source;
    in
    builtins.fetchGit {
      url = builtins.elemAt source 0;
      rev = builtins.elemAt source 1;
      allRefs = true;
    };
in
rustPlatform.buildRustPackage {
  pname = "patchwork";
  version = (lib.importTOML (root + "/Cargo.toml")).workspace.package.version;

  src = fs.toSource {
    inherit root;
    fileset = fs.unions [
      (root + "/Cargo.toml")
      (root + "/Cargo.lock")
      (root + "/crates")
      # Built in: the default registry (patchwork-core) and its themes
      # (patchwork-gui).
      (root + "/registry")
      (root + "/vendor")
      (root + "/LICENSE")
    ];
  };

  cargoLock = {
    lockFile = root + "/Cargo.lock";
    # Fetches git dependencies by the revisions in Cargo.lock, so they need no
    # hashes here. (nixpkgs doesn't allow this; a package there lists
    # outputHashes instead.)
    allowBuiltinFetchGit = true;
  };

  # The CLI, and the app built as `cargo tauri build` would: custom-protocol
  # serves ui/ from the binary instead of a dev server.
  cargoBuildFlags = [
    "--package=patchwork-cli"
    "--package=patchwork-gui"
    "--features=tauri/custom-protocol"
  ];

  # CI runs the tests; here they'd need network access and a home directory.
  doCheck = false;

  nativeBuildInputs = [
    pkg-config
    copyDesktopItems
  ]
  ++ lib.optionals stdenv.hostPlatform.isLinux [ wrapGAppsHook3 ];

  buildInputs = [
    openssl
  ]
  ++ lib.optionals stdenv.hostPlatform.isLinux [
    webkitgtk_4_1
    gtk3
    libsoup_3
    glib-networking
    librsvg
  ];

  env = {
    # Updates come from Nix, not from patchwork; see docs/packaging.md.
    PATCHWORK_PACKAGE_MANAGER = "Nix";
  };

  # Both binaries end up in bin/, where the app looks for the CLI (it's next
  # to the app in our own bundles too).
  desktopItems = lib.optionals stdenv.hostPlatform.isLinux [
    (makeDesktopItem {
      name = "patchwork";
      desktopName = "Patchwork";
      comment = "A universal mod manager";
      exec = "patchwork-gui %u";
      icon = "patchwork";
      startupWMClass = "patchwork-gui";
      categories = [ "Utility" ];
      # Nexus Mods' "Mod manager download" links.
      mimeTypes = [ "x-scheme-handler/nxm" ];
    })
  ];

  preBuild = ''
    cp ${glacierBin1}/*.json ${glacierBin1}/*.rs "$NIX_BUILD_TOP/$(stripHash "$cargoDeps")/"
  '';

  postInstall = ''
    icons=crates/patchwork-gui/icons
    install -Dm644 $icons/32x32.png $out/share/icons/hicolor/32x32/apps/patchwork.png
    install -Dm644 $icons/128x128.png $out/share/icons/hicolor/128x128/apps/patchwork.png
    install -Dm644 $icons/128x128@2x.png $out/share/icons/hicolor/256x256/apps/patchwork.png
  '';

  meta = {
    description = "A mod manager for 300+ games and every major mod site";
    homepage = "https://github.com/patchwork-mods/patchwork";
    changelog = "https://github.com/patchwork-mods/patchwork/blob/main/CHANGELOG.md";
    # The vendored quickentity-rs (Hitman) is LGPL-3.0-only.
    license = with lib.licenses; [
      mit
      lgpl3Only
    ];
    mainProgram = "patchwork-gui";
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
  };
}
