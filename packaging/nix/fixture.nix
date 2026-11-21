# The test fixture binary, for the virtual machine checks. Built from the
# workspace with the consumer's compiler, like any other package here.
{
  lib,
  rustPlatform,
  pkg-config,
  udev,
}:
rustPlatform.buildRustPackage {
  pname = "ctrlassist-fixture";
  version = (lib.importTOML ../../Cargo.toml).workspace.package.version;

  # An explicit list, so a change to docs or packaging doesn't rebuild it.
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.toml
      ../../Cargo.lock
      ../../.cargo
      ../../crates
    ];
  };
  cargoLock.lockFile = ../../Cargo.lock;

  cargoBuildFlags = [
    "--package"
    "ctrlassist-test"
  ];
  # The workspace's tests run in the dev shell and in CI, not here.
  doCheck = false;

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ udev ];

  meta.mainProgram = "ctrlassist-fixture";
}
