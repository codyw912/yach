# hk (https://hk.jdx.dev), packaged from the upstream release binary.
#
# Pinned by scripts/update-hk, which rewrites `version` and every hash here
# together with the hk Config package version in .hk/base/*.pkl. Do not edit
# the hashes by hand; run `bash scripts/update-hk [vX.Y.Z]` and commit.
#
# Upstream ships no x86_64-apple-darwin build, so Intel Macs have no package.
{ pkgs }:

let
  version = "2.6.0";
  assets = {
    x86_64-linux = {
      target = "x86_64-unknown-linux-musl";
      hash = "sha256-zX124BTnCfjKx2Djy2UAMFuhBpFfw7R2TYiqaaDDqS0=";
    };
    aarch64-linux = {
      target = "aarch64-unknown-linux-musl";
      hash = "sha256-X2WF68jeuK5YHAEnaTmC2m3Ka06vUt/VX4gtW7yjmK4=";
    };
    aarch64-darwin = {
      target = "aarch64-apple-darwin";
      hash = "sha256-QKM255M5fVlAsoSq5nRHfUMLJNyGFnvpLggBINgOQvI=";
    };
  };
  system = pkgs.stdenv.hostPlatform.system;
  asset = assets.${system} or (throw "nix/hk.nix: hk ${version} has no release binary for ${system}");
in
pkgs.stdenvNoCC.mkDerivation {
  pname = "hk";
  inherit version;
  src = pkgs.fetchurl {
    url = "https://github.com/jdx/hk/releases/download/v${version}/hk-${asset.target}.tar.gz";
    inherit (asset) hash;
  };
  sourceRoot = ".";
  dontConfigure = true;
  dontBuild = true;
  installPhase = ''
    runHook preInstall
    install -Dm755 hk "$out/bin/hk"
    runHook postInstall
  '';
  meta = {
    description = "Git hook and project check runner";
    homepage = "https://hk.jdx.dev";
    license = pkgs.lib.licenses.mit;
    mainProgram = "hk";
    platforms = builtins.attrNames assets;
  };
}
