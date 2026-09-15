{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };


  outputs = { self, nixpkgs, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
      forEachSystem = f: builtins.listToAttrs (map (name: { inherit name; value = f name; }) systems);
      mkPkgs = system: import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
      cargoProject = builtins.pathExists ./Cargo.toml;
    in
    {
      packages = forEachSystem (system:
        let
          pkgs = mkPkgs system;
          src = pkgs.lib.cleanSourceWith {
            src = self;
            filter = path: type:
              let
                base = builtins.baseNameOf path;
              in
                !(pkgs.lib.elem base [
                  ".git"
                  ".direnv"
                  ".devenv"
                  "target"
                  "result"
                  "result-x86_64-linux-orb"
                  ".devcontainer.json"
                  ".pre-commit-config.yaml"
                ] || pkgs.lib.hasPrefix "result-" base);
          };
        in
        if cargoProject then
          {
            default = pkgs.rustPlatform.buildRustPackage {
              pname = "app";
              version = "0.1.0";
              inherit src;
              cargoLock.lockFile = ./Cargo.lock;
              nativeBuildInputs = with pkgs; [ pkg-config ];
              buildInputs = with pkgs; [ openssl ]
                ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ libiconv zlib ];
            };
          }
        else
          { }
      );

    };
}
