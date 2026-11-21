# One workspace crate's binaries as a package, built with the consumer's
# compiler. Used for the pieces the virtual machine checks need before the
# full package exists.
{
  lib,
  rustPlatform,
  pkg-config,
  udev,
  # The crate to build, and the binary to call its main program.
  crate,
  mainProgram,
  # systemd units to install beside it, with their program paths pointed
  # at this package.
  systemdUnits ? [ ],
}:
rustPlatform.buildRustPackage {
  pname = mainProgram;
  version = (lib.importTOML ../../Cargo.toml).workspace.package.version;

  # An explicit list, so a change to docs or packaging doesn't rebuild it.
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.toml
      ../../Cargo.lock
      ../../.cargo
      ../../crates
      ../../vendor
    ];
  };
  cargoLock.lockFile = ../../Cargo.lock;

  cargoBuildFlags = [
    "--package"
    crate
  ];
  # The workspace's tests run in the dev shell and in CI, not here.
  doCheck = false;

  nativeBuildInputs = [ pkg-config ];
  # libudev, and libsystemd for the hiding service's session check.
  buildInputs = [ udev ];

  postInstall = lib.concatMapStrings (unit: ''
    install -Dm644 ${unit} $out/lib/systemd/system/${baseNameOf unit}
    substituteInPlace $out/lib/systemd/system/${baseNameOf unit} --replace-quiet /usr/bin/ $out/bin/
  '') systemdUnits;

  meta.mainProgram = mainProgram;
}
