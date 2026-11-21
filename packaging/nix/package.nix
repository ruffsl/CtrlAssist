# CtrlAssist as a package: the `ctrlassist` command, which is also the
# daemon and the tray, the hiding service, their systemd units, the udev
# rule, and shell completions.
{
  lib,
  rustPlatform,
  pkg-config,
  udev,
  installShellFiles,
  # Cargo profile for the shipped binaries. `release` builds fastest, for
  # hosts that compile it themselves (the module's default package); the
  # flake's own package passes `dist`, the smaller profile (Cargo.toml has
  # both).
  cargoProfile ? "release",
}:

rustPlatform.buildRustPackage {
  pname = "ctrlassist";
  # The workspace version, so a release bump needs no edit here.
  version = (lib.importTOML ../../Cargo.toml).workspace.package.version;

  buildType = cargoProfile;

  # Only what the build reads, so a change to docs doesn't rebuild it.
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.toml
      ../../Cargo.lock
      ../../.cargo
      ../../crates
      ../../vendor
      ../../packaging/systemd
      ../../packaging/udev
    ];
  };
  cargoLock.lockFile = ../../Cargo.lock;

  # Which crates are built comes from `workspace.default-members` in the
  # root Cargo.toml, the shipping binaries, so the list lives in one place.

  # The workspace's tests run in the dev shell and in CI, not here; the
  # virtual machine checks run the package itself.
  doCheck = false;

  nativeBuildInputs = [
    pkg-config
    # Provides installShellCompletion, used in postInstall.
    installShellFiles
  ];
  # libudev, and libsystemd for the hiding service's session check.
  buildInputs = [ udev ];

  # The units name /usr/bin, right for a deb or an rpm; here they are
  # pointed at this package. `--replace-fail` fails the build if a unit's
  # ExecStart drifts from that, instead of installing one that can't start.
  postInstall = ''
    for unit in ctrlassist-hider.socket ctrlassist-hider.service; do
      install -Dm0644 packaging/systemd/$unit -t $out/lib/systemd/system/
    done
    for unit in ctrlassist-daemon.service ctrlassist-tray.service; do
      install -Dm0644 packaging/systemd/$unit -t $out/lib/systemd/user/
    done
    substituteInPlace \
      $out/lib/systemd/system/ctrlassist-hider.service \
      $out/lib/systemd/user/ctrlassist-daemon.service \
      $out/lib/systemd/user/ctrlassist-tray.service \
      --replace-fail /usr/bin/ $out/bin/

    install -Dm0644 packaging/udev/70-ctrlassist.rules -t $out/lib/udev/rules.d/

    # Shell completions: what registers the command with each shell,
    # printed by the command itself. The registration calls back a bare
    # `ctrlassist`, which resolves on the user's PATH.
    installShellCompletion --cmd ctrlassist \
      --bash <(COMPLETE=bash $out/bin/ctrlassist) \
      --zsh <(COMPLETE=zsh $out/bin/ctrlassist) \
      --fish <(COMPLETE=fish $out/bin/ctrlassist)
  '';

  meta = {
    description = "Controller assist for gaming on Linux: several game controllers as one, or one as several";
    homepage = "https://github.com/CtrlAssist/CtrlAssist";
    changelog = "https://github.com/CtrlAssist/CtrlAssist/releases";
    license = lib.licenses.asl20;
    maintainers = [
      {
        name = "ruffsl";
        github = "ruffsl";
      }
    ];
    platforms = lib.platforms.linux;
    mainProgram = "ctrlassist";
  };
}
