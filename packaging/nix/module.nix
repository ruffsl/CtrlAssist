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

    # Units as packaged. NixOS doesn't act on a packaged unit's [Install]
    # section, so what each is wanted by is said again here.
    systemd.packages = [ cfg.package ];
    systemd.sockets.ctrlassist-hider = lib.mkIf cfg.hiding {
      wantedBy = [ "sockets.target" ];
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
