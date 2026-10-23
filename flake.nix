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

            env = {
              # Required by rust-analyzer
              RUST_SRC_PATH = "${pkgs.rustToolchain}/lib/rustlib/src/rust/library";
            }
            // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              # So patchwork-gui can find GTK/WebKit at runtime, and TLS
              # works inside the webview.
              LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
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
                  # The folder picker (rfd's xdg-portal backend) links it.
                  wayland
                  # libstdc++, for DirectXTex (HITMAN texture conversion).
                  stdenv.cc.cc.lib
                ]
              );
              GIO_MODULE_DIR = "${pkgs.glib-networking}/lib/gio/modules/";
            };
          };
        }
      );

      formatter = forEachSupportedSystem ({ pkgs, ... }: pkgs.nixfmt);
    };
}
