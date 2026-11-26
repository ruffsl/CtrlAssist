# `slint-viewer` at the workspace's Slint version (root Cargo.toml
# [workspace.dependencies] → the exact `slint` pin), so its headless
# `--check` / `--screenshot` renders compile the UI with the compiler the
# app builds with. nixpkgs' is behind the workspace. Mirrors the nixpkgs
# recipe (pkgs/by-name/sl/slint-viewer) with the version bumped.
{
  lib,
  rustPlatform,
  fetchCrate,

  fontconfig,
  libGL,
  pkg-config,
  qt6,
  rust-jemalloc-sys,
}:
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "slint-viewer";
  version = "1.19.0";

  src = fetchCrate {
    inherit (finalAttrs) pname version;
    hash = "sha256-1oazE2WEuEgRr6tJVBy89VqPc3A0jZ+qCYdtgx+NCwg=";
  };

  cargoHash = "sha256-UyYmXHejolkBuN9Q5pxyvfBCuaqkq3wSq19JMhrXlxA=";

  buildInputs = [
    qt6.qtbase
    qt6.qtsvg
    fontconfig
    libGL
    # nixpkgs' cargo setup hook points tikv-jemalloc-sys at the system
    # jemalloc, which this provides (as nixpkgs' own recipe does).
    rust-jemalloc-sys
  ];

  nativeBuildInputs = [
    pkg-config
    qt6.wrapQtAppsHook
  ];

  # There are no tests in the crate.
  doCheck = false;

  meta = {
    description = "Viewer for .slint files (at the workspace's slint version)";
    mainProgram = "slint-viewer";
    homepage = "https://crates.io/crates/slint-viewer";
    license = lib.licenses.gpl3Only;
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
  };
})
