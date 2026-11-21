# CtrlAssist on NixOS: the package on the path, the hiding service's
# socket, the udev rule that gives the seat user uinput and uhid, the
# daemon and the tray as user services, and the real-time limit the
# daemon's input thread asks for.
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
        running setup absent to games. It runs as root, started on demand
        from a socket, and serves only the user at the seat. Without it
        the daemon grabs the pads instead, which hides them from most
        games but not from a program that reads them some other way, as
        Steam reads a PlayStation pad.
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
    ];
    services.udev.packages = [ cfg.package ];

    # Units as packaged. NixOS doesn't act on a packaged unit's [Install]
    # section, so what each is wanted by is said again here.
    systemd.packages = [ cfg.package ];
    systemd.sockets.ctrlassist-hider = lib.mkIf cfg.hiding {
      wantedBy = [ "sockets.target" ];
    };
    # For udevadm, which reloads the rules the service writes.
    systemd.services.ctrlassist-hider = lib.mkIf cfg.hiding {
      path = [ config.systemd.package ];
    };
    systemd.user.services.ctrlassist-daemon.wantedBy = [ "default.target" ];
    systemd.user.services.ctrlassist-tray = lib.mkIf cfg.tray {
      wantedBy = [ "graphical-session.target" ];
    };

    # A user service's limits can't exceed its service manager's, so the
    # manager is given what the daemon's unit asks for.
    systemd.services."user@".serviceConfig = lib.mkIf cfg.realtime {
      LimitRTPRIO = 10;
    };
  };
}
