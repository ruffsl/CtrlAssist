# The flake's outputs for plain Nix, through flake-compat (pinned in
# flake.lock): `nix-build -A default` builds the package, and `import` of
# this directory gives `nixosModules.default` and `packages.<system>`.
(import (
  let
    lock = builtins.fromJSON (builtins.readFile ./flake.lock);
    node = lock.nodes.${lock.nodes.root.inputs.flake-compat}.locked;
  in
  fetchTarball {
    url = "https://github.com/${node.owner}/${node.repo}/archive/${node.rev}.tar.gz";
    sha256 = node.narHash;
  }
) { src = ./.; }).defaultNix
