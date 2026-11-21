# Three ways a user's systemd service might be allowed real-time
# scheduling, each on a machine of its own, asked the same question: may a
# service of alice's run a thread first-in-first-out at priority 10?
#
#   nix build --impure --expr 'import ./spikes/realtime/test.nix { }' -L
{
  # The nixpkgs the flake pins, read from its lock.
  pkgs ?
    let
      locked = (builtins.fromJSON (builtins.readFile ../../flake.lock)).nodes.nixpkgs.locked;
    in
    import (builtins.fetchTree locked) { },
}:
let
  common = {
    users.users.alice = {
      isNormalUser = true;
      uid = 1000;
      # Her service manager runs without her logging in.
      linger = true;
    };
    environment.systemPackages = [ pkgs.util-linux ];
  };
  ask = pkgs.writers.writePython3Bin "ask-rtkit" {
    libraries = [ pkgs.python3Packages.dbus-python ];
    doCheck = false;
  } (builtins.readFile ./ask.py);
in
pkgs.testers.runNixOSTest {
  name = "ctrlassist-spike-realtime";

  # Nothing raised: what a stock machine gives.
  nodes.stock = common;

  # The service manager's own limit raised by a drop-in.
  nodes.dropin = {
    imports = [ common ];
    systemd.services."user@".serviceConfig.LimitRTPRIO = 10;
  };

  # A PAM limit for the user.
  nodes.pam = {
    imports = [ common ];
    security.pam.loginLimits = [
      {
        domain = "alice";
        item = "rtprio";
        type = "-";
        value = "10";
      }
    ];
  };

  # rtkit, asked over D-Bus by the process itself.
  nodes.rtkit = {
    imports = [ common ];
    security.rtkit.enable = true;
    environment.systemPackages = [ ask ];
  };

  # The same, with the user logged in at the console.
  nodes.rtkitseat = {
    imports = [ common ];
    security.rtkit.enable = true;
    environment.systemPackages = [ ask ];
    services.getty.autologinUser = "alice";
  };

  testScript = ''
    start_all()

    def in_service(machine, command, limit=True):
        """Runs a command as a service of alice's, with the unit asking for the limit or not."""
        prop = "-p LimitRTPRIO=10 " if limit else ""
        return machine.succeed(
            "sudo -u alice XDG_RUNTIME_DIR=/run/user/1000 "
            f"systemd-run --user --wait --pipe --quiet {prop}sh -c '{command}'"
        ).strip()

    probe = "echo limit=$(ulimit -r); chrt -f 10 true 2>/dev/null && echo fifo=granted || echo fifo=refused"

    for machine in (stock, dropin, pam, rtkit, rtkitseat):
        machine.wait_for_unit("multi-user.target")
        machine.wait_until_succeeds("test -S /run/user/1000/bus")

    for machine in (stock, dropin, pam):
        for limit in (True, False):
            out = in_service(machine, probe, limit).replace("\n", " ")
            print(f"FINDING {machine.name} unit-asks={limit}: {out}")
        shell = machine.succeed("su -l alice -c 'ulimit -r'").strip()
        print(f"FINDING {machine.name} login-shell limit={shell}")
        manager = machine.succeed("grep -i 'realtime priority' /proc/$(pgrep -u alice -x systemd | head -1)/limits").strip()
        print(f"FINDING {machine.name} manager: {manager}")

    rtkitseat.wait_until_succeeds("loginctl list-sessions | grep alice")
    # rtkit is started by the first call to it.
    for machine in (rtkit, rtkitseat):
        for arguments in ("10", "10 200000", "10 100000", "25 200000", "10 300000"):
            out = in_service(machine, f"ask-rtkit {arguments}", limit=False)
            print(f"FINDING {machine.name}: {out}")
    # Granted, a thread that then spins without blocking runs into the limit
    # it had to set.
    status, out = rtkitseat.execute(
        "sudo -u alice XDG_RUNTIME_DIR=/run/user/1000 timeout 30 "
        "systemd-run --user --wait --pipe --quiet -u spin ask-rtkit 10 200000 spin 2>&1"
    )
    how = rtkitseat.succeed(
        "journalctl _SYSTEMD_USER_UNIT=spin.service -o cat --no-pager; "
        "sudo -u alice XDG_RUNTIME_DIR=/run/user/1000 systemctl --user show spin.service -p Result -p ExecMainStatus -p ExecMainCode || true"
    ).strip().replace("\n", " | ")
    print(f"FINDING rtkitseat spin: exit={status} out={out.strip()!r} unit: {how}")
  '';
}
