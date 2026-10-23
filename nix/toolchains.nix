{
  pkgs,
  fenix,
  system,
}:

let
  inherit (pkgs) lib;

  # Rust 1.97.0 + wasm32-unknown-unknown, matching .github/workflows/ci.yml and
  # rust-toolchain.toml (channel = "stable", but CI pins the exact 1.97.0
  # toolchain; rust-toolchain.toml's "stable" would otherwise float). The
  # manifest hash is from
  # https://static.rust-lang.org/dist/channel-rust-1.97.0.toml.sha256
  rustChannel = fenix.packages.${system}.fromToolchainName {
    name = "1.97.0";
    sha256 = "sha256-OATSZm98Es5kIFuqaba+UvkQtFsVgJEBMmS+t6od5/U=";
  };
  rustWasmStd = fenix.packages.${system}.targets.wasm32-unknown-unknown.fromToolchainName {
    name = "1.97.0";
    sha256 = "sha256-OATSZm98Es5kIFuqaba+UvkQtFsVgJEBMmS+t6od5/U=";
  };
  rust-toolchain = fenix.packages.${system}.combine [
    rustChannel.rustc
    rustChannel.cargo
    rustChannel.rust-std
    rustChannel.rust-src
    rustChannel.clippy-preview
    rustChannel.rustfmt-preview
    rustWasmStd.rust-std
  ];

  # The nightly pinned by .github/workflows/fuzz.yml
  # (LESSDUGIN_FUZZ_TOOLCHAIN: nightly-2026-10-22), used only for cargo-fuzz
  # sanitizer builds; the xtask itself stays on the stable pin above.
  # withComponents (not .toolchain) so only rustc/cargo/std get built — the
  # full combine would drag in miri, cranelift, rust-analyzer, docs etc.
  rust-nightly = (fenix.packages.${system}.fromToolchainName {
    name = "nightly-2026-10-22";
    sha256 = "sha256-V6dLt89/mszjKbYHRZ2s6fSfFTvoMrTv9o1Y4BDCMWk=";
  }).withComponents [ "rustc" "cargo" "rust-std" ];

  # wasm-bindgen-cli pinned to the exact workspace `wasm-bindgen = "=0.2.129"`
  # pin (xtask/src/main.rs::wasm_bindgen_version); nixpkgs currently ships
  # 0.2.127 as its default alias, so build it explicitly from crates.io.
  wasm-bindgen-cli = pkgs.buildWasmBindgenCli {
    src = pkgs.fetchCrate {
      pname = "wasm-bindgen-cli";
      version = "0.2.129";
      hash = "sha256-pcecKQd7E8Opw6bkFoE569epUi7gh5qpQF1e5PJY6V8=";
    };
    cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
      pname = "wasm-bindgen-cli";
      version = "0.2.129";
      src = pkgs.fetchCrate {
        pname = "wasm-bindgen-cli";
        version = "0.2.129";
        hash = "sha256-pcecKQd7E8Opw6bkFoE569epUi7gh5qpQF1e5PJY6V8=";
      };
      hash = "sha256-vmUrWVU7kPJJxO5qIVeAkwQyWDELO1Z4Z5gitz2kco8=";
    };
  };

  # goat (github.com/bluesky-social/goat) pinned to v0.2.6, the version
  # .github/workflows/ci.yml installs. goat v0.2.6 requires Go >= 1.27 (its
  # go.mod says `go 1.27`); nixpkgs' packaged atproto-goat is 0.2.4, so build
  # v0.2.6 directly against go_1_27.
  goat = pkgs.buildGo127Module {
    pname = "goat";
    version = "0.2.6";
    src = pkgs.fetchFromGitHub {
      owner = "bluesky-social";
      repo = "goat";
      tag = "v0.2.6";
      hash = "sha256-/rqb081FY6hvenVnwVdzPhkKfZ2oGKsvALibaTCLDSw=";
    };
    vendorHash = "sha256-zWIDQV7rsEc8h8G/Bu8tZKs76mX9zMkaLn4PcPYNR/M=";
    # versioninfo.Short() reads VCS build info at runtime, which is absent in a
    # Nix build; pin it to the package version like nixpkgs' atproto-goat does.
    postPatch = ''
      substituteInPlace main.go \
        --replace-fail "versioninfo.Short()" '"0.2.6"' \
        --replace-fail '"github.com/earthboundkid/versioninfo/v2"' ""
    '';
    subPackages = [ "." ];
    meta.mainProgram = "goat";
  };

  # pnpm 10.28.0, the exact version named by package.json's packageManager
  # field (corepack would otherwise fetch a generic-linux binary that cannot
  # run on NixOS).
  pnpm = pkgs.callPackage "${pkgs.path}/pkgs/development/tools/pnpm/generic.nix" {
    version = "10.28.0";
    hash = "sha256-mwsE5ueZRVZpF/hBG7b2X9Lz4VkEJpBOhQDhrMSzNWE=";
    nodejs = null;
  };
in
{
  inherit rust-toolchain rust-nightly wasm-bindgen-cli goat pnpm;
}
