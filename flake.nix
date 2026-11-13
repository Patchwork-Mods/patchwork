{
  description = "A Nix-flake-based Rust development environment";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1"; # unstable Nixpkgs
    fenix = {
      url = "https://flakehub.com/f/nix-community/fenix/0.1";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, ... }@inputs:

    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forEachSupportedSystem =
        f:
        inputs.nixpkgs.lib.genAttrs supportedSystems (
          system:
          f {
            inherit system;
            pkgs = import inputs.nixpkgs {
              inherit system;
              overlays = [
                inputs.self.overlays.default
              ];
            };
          }
        );
    in
    {
      overlays.default = final: prev: {
        rustToolchain =
          with inputs.fenix.packages.${prev.stdenv.hostPlatform.system};
          combine (
            with stable;
            [
              clippy
              rustc
              cargo
              rustfmt
              rust-src
            ]
          );
      };

      devShells = forEachSupportedSystem (
        { pkgs, system }:
        {
          default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                rustToolchain
                openssl
                pkg-config
                cargo-deny
                cargo-edit
                cargo-watch
                rust-analyzer
                cargo-dist
                # `cargo tauri`, for bundling the app and its updater signing key.
                cargo-tauri
                self.formatter.${system}
              ]
              # For the Tauri GUI (patchwork-gui).
              ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
                webkitgtk_4_1
                gtk3
                libsoup_3
                glib-networking
                librsvg
                wayland
              ];

            env =
              let
                # What patchwork-gui loads at runtime: GTK/WebKit, the folder
                # picker's (rfd's xdg-portal backend) wayland, and libstdc++
                # for DirectXTex (HITMAN texture conversion).
                guiLibs = pkgs.lib.makeLibraryPath (
                  with pkgs;
                  [
                    webkitgtk_4_1
                    gtk3
                    libsoup_3
                    glib
                    cairo
                    pango
                    gdk-pixbuf
                    harfbuzz
                    atk
                    dbus
                    wayland
                    stdenv.cc.cc.lib
                  ]
                );
                gioModules = "${pkgs.glib-networking}/lib/gio/modules/";
              in
              {
                # Required by rust-analyzer
                RUST_SRC_PATH = "${pkgs.rustToolchain}/lib/rustlib/src/rust/library";
              }
              // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
                # So patchwork-gui can find GTK/WebKit at runtime, and TLS
                # works inside the webview.
                LD_LIBRARY_PATH = guiLibs;
                GIO_MODULE_DIR = gioModules;
                # The same, built into patchwork-gui (see its build.rs), so
                # it also runs outside this shell: when the browser starts
                # it for an nxm:// link, say.
                PATCHWORK_GUI_RPATH = guiLibs;
                PATCHWORK_GUI_GIO_MODULE_DIR = gioModules;
              };
          };
        }
      );

      formatter = forEachSupportedSystem ({ pkgs, ... }: pkgs.nixfmt);
    };
}
