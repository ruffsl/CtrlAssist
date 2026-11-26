# cargo-about, pinned to one release. Some releases ignore the Slint crates'
# LicenseRef-* clarify files (EmbarkStudios/cargo-about#303 and #329) and
# drop their royalty-free license text from THIRD-PARTY-LICENSES.html with
# only a "no license file" warning, which doesn't fail the build. So bump
# `version` only after generating the bundle with the new release and
# checking the Slint crates keep their license text.
#
# nixpkgs' copy passes through when it's this version; otherwise this
# builds it from source.
{
  cargo-about,
  fetchFromGitHub,
  rustPlatform,
}:

let
  version = "0.9.1";
in
if cargo-about.version == version then
  cargo-about
else
  let
    src = fetchFromGitHub {
      owner = "EmbarkStudios";
      repo = "cargo-about";
      tag = version;
      hash = "sha256-mZyfECnlpYom/XI445DOeUAthqK3HKC+QkkK+5gf0/E=";
    };
  in
  cargo-about.overrideAttrs {
    inherit version src;
    cargoDeps = rustPlatform.fetchCargoVendor {
      inherit src;
      hash = "sha256-//t3ASNljC4rosk5oOmcJy8PZKRYjlQ0euzzAmfiSPQ=";
    };
    # Upstream's own suite: a build tool's tests, run on every release
    # build here since this copy isn't in any binary cache.
    doCheck = false;
  }
