# One workspace crate's binaries as a package, built with the consumer's
# compiler. Used for the pieces the virtual machine checks need before the
# full package exists.
{
  lib,
  rustPlatform,
  pkg-config,
  udev,
  coreutils,
  systemd,
  # The crate to build, and the binary to call its main program.
  crate,
  mainProgram,
  # systemd units to install beside it, with their program paths pointed
  # at this package.
  systemdUnits ? [ ],
  # udev rules to install beside it, with the programs they run pointed
  # at the store.
  udevRules ? [ ],
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
    ];
  };
  cargoLock = {
    lockFile = ../../Cargo.lock;
    # The one dependency from git: evdev from the project's fork, at the
    # commit the root Cargo.toml pins. Changed with that pin.
    outputHashes."evdev-0.13.2" = "sha256-/bezu9rIIBthpGEuhalEvl1FoN43Ro4Q3oulDt2e7XU=";
  };

  cargoBuildFlags = [
    "--package"
    crate
  ];
  # The workspace's tests run in the dev shell and in CI, not here.
  doCheck = false;

  nativeBuildInputs = [ pkg-config ];
  # libudev, and libsystemd for the hiding service's session check.
  buildInputs = [ udev ];

  postInstall =
    lib.concatMapStrings (unit: ''
      install -Dm644 ${unit} $out/lib/systemd/system/${baseNameOf unit}
      substituteInPlace $out/lib/systemd/system/${baseNameOf unit} --replace-quiet /usr/bin/ $out/bin/
    '') systemdUnits
    + lib.concatMapStrings (rule: ''
      install -Dm644 ${rule} $out/lib/udev/rules.d/${baseNameOf rule}
      substituteInPlace $out/lib/udev/rules.d/${baseNameOf rule} \
        --replace-quiet /usr/bin/udevadm ${systemd}/bin/udevadm \
        --replace-quiet /usr/bin/ ${coreutils}/bin/
    '') udevRules;

  meta.mainProgram = mainProgram;
}
