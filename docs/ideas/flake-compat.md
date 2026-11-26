# A `default.nix` for Nix without flakes

TisVlay has a root `default.nix` that reads the flake through flake-compat, so a NixOS configuration without flakes can import the package and the module. CtrlAssist has only the flake.

**Why deferred.** Nobody has asked, and the installation page documents the flake alone.

**Would bring it in.** A user on channels or npins asking how to install. The file is TisVlay's with the name changed, and the installation page gains a section.
