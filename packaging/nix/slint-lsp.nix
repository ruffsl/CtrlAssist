# `slint-lsp` at the workspace's Slint version, for the editor
# integrations' live preview of the UI: an LSP older than the edition in
# the crate's `slint-project.json` can't read the project file. nixpkgs'
# own recipe (pkgs/by-name/sl/slint-lsp) on the crates.io release; the
# file goes when nixpkgs' slint-lsp reaches the workspace's pin.
{
  lib,
  fetchCrate,
  rustPlatform,
  slint-lsp,
}:
slint-lsp.overrideAttrs (finalAttrs: _: {
  version = "1.19.0";
  src = fetchCrate {
    pname = "slint-lsp";
    inherit (finalAttrs) version;
    hash = "sha256-6KOlnDcaurRsXdQmgcyjLPyDMFpe+D1CgJPyFtiKb78=";
  };
  cargoDeps = rustPlatform.fetchCargoVendor {
    inherit (finalAttrs) src;
    hash = "sha256-x+DrL82HT6r2uubapJE3Yig6tcgRexzI0HtNI3az++w=";
  };
})
