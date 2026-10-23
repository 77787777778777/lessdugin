{
  description = "lessdugin development and CI toolchain";

  nixConfig = {
    extra-substituters = [ "https://nix-community.cachix.org" ];
    extra-trusted-public-keys = [
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    ];
  };

  inputs = {
    # Pinned to the running system's nixpkgs rev so the dev shell reuses the
    # warm binary cache (no local rebuilds of stdenv-class packages).
    nixpkgs.url = "github:NixOS/nixpkgs?rev=d2346664acd94bacb4e69eee02e238ea40c726ef";
    fenix = {
      url = "github:nix-community/fenix?rev=6bd8c2ef0033a19933ce0d0a4152a7d204e430d3";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      fenix,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          tools = import ./nix/toolchains.nix { inherit pkgs fenix system; };
        in
        {
          inherit (tools) goat wasm-bindgen-cli rust-toolchain rust-nightly pnpm;
          playwright-browsers = pkgs.callPackage ./nix/playwright { inherit system; };
          default = tools.rust-toolchain;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          tools = import ./nix/toolchains.nix { inherit pkgs fenix system; };
          playwrightBrowsers = self.packages.${system}.playwright-browsers;

          # rustup comes FIRST so its `cargo`/`rustc` proxies shadow everything
          # else: `cargo +nightly-2026-10-22 fuzz` (xtask fuzz / cargo-fuzz)
          # only works through the rustup proxy. The shellHook links the exact
          # pinned toolchains (below) into $RUSTUP_HOME and RUSTUP_TOOLCHAIN
          # pins the default, so nothing is ever downloaded into $HOME.
          # gcc-based stdenv deliberately: CI runs on ubuntu-latest with gcc,
          # so bundled-C builds (libsqlite3-sys) match CI.
          commonPackages = with pkgs; [
            rustup
            tools.rust-toolchain
            tools.rust-nightly
            tools.goat
            tools.wasm-bindgen-cli
            tools.pnpm
            nodejs_22
            binaryen
            cargo-deny
            cargo-nextest
            pkg-config
            clang
            lld
            git
            jq
          ];

          shellSetup = ''
            export RUSTUP_HOME="''${RUSTUP_HOME:-$PWD/.rustup}"
            export RUSTUP_TOOLCHAIN=1.97.0-x86_64-unknown-linux-gnu
            # Purity: never fetch toolchains from the network; everything is
            # symlinked from the Nix store below. A missing +toolchain must be
            # a loud error, not a silent auto-download.
            export RUSTUP_AUTO_INSTALL=0
            # `rustup toolchain link` rejects version-number names like 1.97.0,
            # and `+nightly-2026-10-22` would otherwise auto-install from the
            # network (host triple gets appended to channel names), so register
            # both toolchains as the exact symlinks rustup itself would create.
            mkdir -p "$RUSTUP_HOME/toolchains"
            ln -sfn ${tools.rust-toolchain} "$RUSTUP_HOME/toolchains/1.97.0-x86_64-unknown-linux-gnu"
            ln -sfn ${tools.rust-nightly} "$RUSTUP_HOME/toolchains/nightly-2026-10-22-x86_64-unknown-linux-gnu"
            # `cargo +nightly-2026-10-22 fuzz` (xtask fuzz / cargo-fuzz) needs
            # rustup's proxies in front of everything: the proxies resolve the
            # +toolchain directive and exec the linked store toolchain. mkShell
            # puts the packages' bin dirs (including rustup's proxies) on PATH
            # after this hook, so prepend the proxy dir explicitly to be safe.
            export PATH="${pkgs.rustup}/bin:$PATH"
            # corepack would fetch a generic-linux pnpm binary that cannot run
            # on NixOS; the flake provides the exact pinned pnpm instead.
            export COREPACK_ENABLE_STRICT=0
            export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
          '';

          mkDevShell =
            { full ? false }:
            pkgs.mkShell {
              name = if full then "lessdugin-ci" else "lessdugin";
              packages = commonPackages ++ pkgs.lib.optionals full [ pkgs.cargo-fuzz ];
              shellHook =
                shellSetup
                + pkgs.lib.optionalString full ''
                  # Exact browser artifacts for @playwright/test 1.56.1
                  # (chromium/headless-shell rev 1194, ffmpeg rev 1011 — what
                  # `playwright install` would download). Never substitute the
                  # system browser or a different Playwright driver revision.
                  export PLAYWRIGHT_BROWSERS_PATH=${playwrightBrowsers}
                  export LESSDUGIN_FUZZ_TOOLCHAIN=nightly-2026-10-22
                '';
            };
        in
        {
          default = mkDevShell { };
          ci = mkDevShell { full = true; };
        }
      );
    };
}
