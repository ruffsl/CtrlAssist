# CtrlAssist as a package: the `ctrlassist` command, which is also the
# daemon and the tray, the hiding service, the settings window with its
# desktop entry, their systemd units, the udev rule, and shell
# completions.
{
  lib,
  callPackage,
  rustPlatform,
  pkg-config,
  udev,
  installShellFiles,
  copyDesktopItems,
  makeDesktopItem,
  cargo-about,
  coreutils,
  # Cargo profile for the shipped binaries. `release` builds fastest, for
  # hosts that compile it themselves (the module's default package); the
  # flake's own package passes `dist`, the smaller profile (Cargo.toml has
  # both).
  cargoProfile ? "release",
}:

let
  # What the window loads at run time, which patchelf can't see; the dev
  # shell has the same set.
  rpathLibs = callPackage ./slint-runtime.nix { };
in
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
      ../../packaging/licensing
      ../../packaging/systemd
      ../../packaging/udev
      ../../packaging/sysusers
    ];
  };
  cargoLock.lockFile = ../../Cargo.lock;

  # Which crates are built comes from `workspace.default-members` in the
  # root Cargo.toml, the shipping binaries, so the list lives in one place.

  # The workspace's tests run in the dev shell and in CI, not here; the
  # virtual machine checks run the package itself.
  doCheck = false;

  nativeBuildInputs = [
    # Writes the third-party license bundle in preBuild. The pinned one,
    # or the Slint crates' license text can go missing (./cargo-about.nix).
    (callPackage ./cargo-about.nix { inherit cargo-about; })
    pkg-config
    # Copies `desktopItems` into $out/share/applications.
    copyDesktopItems
    # Provides installShellCompletion, used in postInstall.
    installShellFiles
  ];
  # libudev, and libsystemd for the hiding service's session check; and
  # the window's run-time libraries, some of which it also links.
  buildInputs = [ udev ] ++ rpathLibs;

  # The window's entry in the launcher, as TisVlay's package makes its
  # own: by nixpkgs' helper, which checks it, and kept the same by hand
  # as packaging/freedesktop/io.github.ctrlassist.Gui.desktop, which the
  # deb and rpm ship. `exec` is bare: the package's bin is on the PATH a
  # launcher searches.
  desktopItems = [
    (makeDesktopItem {
      name = "io.github.ctrlassist.Gui";
      desktopName = "CtrlAssist";
      genericName = "Controller Assist";
      comment = "Several game controllers as one, or one as several";
      exec = "ctrlassist-gui";
      tryExec = "ctrlassist-gui";
      icon = "input-gaming";
      categories = [
        "Settings"
        "HardwareSettings"
      ];
      keywords = [
        "gamepad"
        "controller"
        "joystick"
        "assist"
        "copilot"
      ];
      startupNotify = true;
      startupWMClass = "io.github.ctrlassist.Gui";
      dbusActivatable = true;
      singleMainWindow = true;
      implements = [ "org.freedesktop.Application" ];
    })
  ];

  # cargo-about reads only the vendored sources, so it runs in the
  # sandbox with no network.
  preBuild = ''
    cargo about generate \
      --config packaging/licensing/about.toml \
      --workspace --all-features --locked --fail \
      packaging/licensing/about.html.hbs \
      -o packaging/licensing/THIRD-PARTY-LICENSES.html
  '';

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

    # How the session bus starts the window: a second launch, and the
    # tray, raise the one that is open.
    install -Dm0644 packaging/systemd/io.github.ctrlassist.Gui.service -t $out/share/dbus-1/services/
    substituteInPlace $out/share/dbus-1/services/io.github.ctrlassist.Gui.service \
      --replace-fail 'Exec=/usr/bin/ctrlassist-gui' "Exec=$out/bin/ctrlassist-gui"

    install -Dm0644 packaging/udev/70-ctrlassist.rules -t $out/lib/udev/rules.d/
    # The hiding service's rule runs chgrp and chmod, named by their
    # /usr/bin paths for the other packages.
    install -Dm0644 packaging/udev/72-ctrlassist-hide.rules -t $out/lib/udev/rules.d/
    substituteInPlace $out/lib/udev/rules.d/72-ctrlassist-hide.rules \
      --replace-fail /usr/bin/chgrp ${coreutils}/bin/chgrp \
      --replace-fail /usr/bin/chmod ${coreutils}/bin/chmod
    # The hiding service's user, for systemd-sysusers where the package
    # is installed by other means than the NixOS module, which makes the
    # user itself.
    install -Dm0644 packaging/sysusers/ctrlassist.conf -t $out/lib/sysusers.d/

    install -Dm0644 packaging/licensing/NOTICE -t $out/share/doc/ctrlassist/
    install -Dm0644 packaging/licensing/THIRD-PARTY-LICENSES.html -t $out/share/doc/ctrlassist/

    # Shell completions: what registers the command with each shell,
    # printed by the command itself. The registration calls back a bare
    # `ctrlassist`, which resolves on the user's PATH.
    installShellCompletion --cmd ctrlassist \
      --bash <(COMPLETE=bash $out/bin/ctrlassist) \
      --zsh <(COMPLETE=zsh $out/bin/ctrlassist) \
      --fish <(COMPLETE=fish $out/bin/ctrlassist)
  '';

  # The libraries the window loads by name go on its rpath by hand, and
  # the usual pass, which would take off what it finds no use of, is
  # skipped.
  postFixup = ''
    patchelf --add-rpath "${lib.makeLibraryPath rpathLibs}" $out/bin/ctrlassist-gui
  '';
  dontPatchELF = true;

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
