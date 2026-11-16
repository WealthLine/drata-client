{
  description = "Rust client for Drata's custom connections and Access Review APIs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      devShells = forAll (
        pkgs:
        let
          # The latest stable as of the rust-overlay revision in flake.lock.
          # `rust-version` in Cargo.toml is the support floor, set separately.
          rust = pkgs.rust-bin.stable.latest.minimal.override {
            extensions = [
              "clippy"
              "rustfmt"
              "rust-src"
            ];
          };

          # rustfmt.toml uses nightly-only options, so `cargo fmt` runs the
          # latest nightly rustfmt through RUSTFMT.
          nightlyRustfmt = pkgs.rust-bin.selectLatestNightlyWith (
            toolchain: toolchain.minimal.override { extensions = [ "rustfmt" ]; }
          );
        in
        {
          default = pkgs.mkShell {
            packages = [
              rust
              pkgs.rust-analyzer
              pkgs.cargo-nextest
              pkgs.cargo-deny
            ];

            env.RUSTFMT = "${nightlyRustfmt}/bin/rustfmt";
          };
        }
      );

      formatter = forAll (pkgs: pkgs.nixfmt);
    };
}
