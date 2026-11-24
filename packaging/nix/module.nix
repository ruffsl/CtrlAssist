# CtrlAssist on NixOS: the package on the path, the hiding service's
# socket and user, the udev rules that give the seat user uinput and uhid
# and the hiding service what it hides pads with, the daemon and the tray
# as user services, and the real-time limit the daemon's input thread
# asks for.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.ctrlassist;
in
{
  options.services.ctrlassist = {
    enable = lib.mkEnableOption "CtrlAssist, controller assist for gaming";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.ctrlassist or (pkgs.callPackage ./package.nix { });
      defaultText = lib.literalExpression "pkgs.ctrlassist";
      description = ''
        The CtrlAssist package to use. Defaults to `pkgs.ctrlassist` when
        the flake's `overlays.default` is applied; otherwise it is built
        from this module's sibling `package.nix`, against the host's
        nixpkgs.
      '';
    };

    hiding = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether to install the hiding service, which makes the pads of a
        running setup absent to games. It runs as a user of its own with
        no capabilities, started on demand from a socket, and serves only
        the user at the seat. Without it the daemon grabs the pads
        instead, which hides them from most games but not from a program
        that reads them some other way, as Steam reads a PlayStation pad.
      '';
    };

    realtime = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether to let users' services ask for real-time scheduling up to
        priority 10, which the daemon's input thread does. It keeps input
        latency dependable when the machine is busy. Without it the
        daemon runs the same, and its state says it was refused.
      '';
    };

    tray = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether to start the system tray with each user's graphical
        session.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    # Virtual pads are made through uinput, which may not be loaded until
    # something asks for it; the rule below needs the node to exist.
    boot.kernelModules = [
      "uinput"
      "uhid"
      # A virtual DualSense is this driver's pad, and the driver has to
      # be there before the pad is.
      "hid-playstation"
    ];
    services.udev.packages = [ cfg.package ];
    # udev applies a rule to a device at the device's next event. A switch
    # to a configuration with new rules restarts udevd on them (nixpkgs'
    # udev module gives systemd-udevd that restart trigger) but sends no
    # device an event, so pads, uinput and uhid already there when the
    # module is enabled or its rules change would go without them until
    # plugged in again or the next boot: the hiding service couldn't ask
    # for a connected pad's re-bind. This asks for the events the
    # packages' install script asks for, once udevd has the new rules and
    # the modules are loaded, at each switch that changes the package.
    systemd.services.ctrlassist-udev-trigger = {
      description = "Apply CtrlAssist's udev rules to devices already connected";
      wantedBy = [ "multi-user.target" ];
      after = [
        "systemd-udevd.service"
        "systemd-udev-trigger.service"
        "systemd-modules-load.service"
      ];
      restartTriggers = [ cfg.package ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart =
          let
            udevadm = "${config.systemd.package}/bin/udevadm";
          in
          [
            "${udevadm} trigger --subsystem-match=misc --sysname-match=uinput --sysname-match=uhid"
            "${udevadm} trigger --action=change --subsystem-match=input --property-match=ID_INPUT_JOYSTICK=1"
            "${udevadm} trigger --action=change --subsystem-match=hidraw"
          ];
      };
    };

    # Units as packaged. NixOS doesn't act on a packaged unit's [Install]
    # section, so what each is wanted by is said again here.
    systemd.packages = [ cfg.package ];
    systemd.sockets.ctrlassist-hider = lib.mkIf cfg.hiding {
      wantedBy = [ "sockets.target" ];
      # Restarted when the unit below is.
      partOf = [ "ctrlassist-hider-socket-unit.service" ];
    };
    # A listener keeps the settings it was made with until its socket is
    # restarted, and a switch restarts no socket whose unit changed:
    # NixOS's switch leaves changed sockets alone, and restarts a socket
    # only with its service, when that service is running and has changed
    # too. So a setting new to the socket's unit, as refusing descriptors
    # once was, would hold only from the next boot. This unit stands for
    # the socket's unit, the package's and what this module adds to it: a
    # switch that changes either restarts this unit, and the restart is
    # passed on to the socket, and from the socket to the service if it
    # is running. Stopping the service shows again what it had hidden, and
    # the daemon takes each pad back into its setup as it comes back:
    # hidden again by the service started afresh, or grabbed if the
    # socket isn't listening again yet, and hidden once it is, as after
    # any switch that changes the package while the service runs, which
    # restarts both itself. The switch that first adds this unit only
    # starts it, and a start isn't passed on (`PartOf=` passes on stops
    # and restarts): a socket already listening from a configuration
    # without this unit keeps its settings until the next boot. No
    # release had the socket without it.
    systemd.services.ctrlassist-hider-socket-unit = lib.mkIf cfg.hiding {
      description = "Restart CtrlAssist's hiding service socket when its unit changes";
      wantedBy = [ "multi-user.target" ];
      restartTriggers = [
        cfg.package
        config.systemd.units."ctrlassist-hider.socket".unit
      ];
      # Restarted, not stopped and started again: only a restart is passed
      # on to the socket as one.
      stopIfChanged = false;
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${pkgs.coreutils}/bin/true";
      };
    };
    # The hiding service's own user, which its unit runs as, and its
    # group, which the package's udev rule names whether or not hiding is
    # on: the rule gives it pads' files, and needs it to exist.
    users.users.ctrlassist-hider = {
      isSystemUser = true;
      group = "ctrlassist-hider";
      description = "CtrlAssist hiding service";
    };
    users.groups.ctrlassist-hider = { };
    systemd.user.services.ctrlassist-daemon.wantedBy = [ "default.target" ];
    systemd.user.services.ctrlassist-tray = lib.mkIf cfg.tray {
      wantedBy = [ "graphical-session.target" ];
    };

    # A user service has the limits its service manager has and can't
    # raise them itself, so the manager is given what the daemon's input
    # thread asks for.
    systemd.services."user@".serviceConfig = lib.mkIf cfg.realtime {
      LimitRTPRIO = 10;
    };
  };
}
