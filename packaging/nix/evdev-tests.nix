# The evdev crate's own tests, its device tests among them, run as root in
# a throwaway machine: they make keyboards through uinput and grab them,
# which is not for a desktop somebody is using. For the fork's branches
# (docs/plans/evdev-upstream.md); `just evdev-test` builds it.
#
# `src` is a checkout of the crate with a Cargo.lock beside its manifest,
# which upstream doesn't keep and the recipe makes.
{ pkgs, src }:
let
  tests = pkgs.rustPlatform.buildRustPackage {
    pname = "evdev-tests";
    version = "0";
    src = builtins.path {
      path = src;
      name = "evdev-src";
      filter = path: _: !(pkgs.lib.hasSuffix "/target" path) && !(pkgs.lib.hasSuffix "/.git" path);
    };
    cargoLock.lockFile = src + "/Cargo.lock";
    doCheck = false;
    buildPhase = ''
      cargo test --offline --no-run --lib --tests --features device-test,tokio 2>&1 | tee build.log
    '';
    # Each test binary, by the path cargo says it built.
    installPhase = ''
      mkdir -p $out/bin
      grep -o 'Executable [^(]* (\([^)]*\))' build.log | sed 's/.*(\(.*\))/\1/' | while read -r binary; do
        cp "$binary" $out/bin/
      done
    '';
  };
in
pkgs.testers.runNixOSTest {
  name = "evdev-tests";
  nodes.machine = {
    boot.kernelModules = [ "uinput" ];
    virtualisation.memorySize = 1024;
  };
  # A test that fails doesn't fail the build: what each binary printed is
  # the result, one file for each.
  testScript = ''
    machine.wait_for_unit("multi-user.target")
    _, listing = machine.execute("ls ${tests}/bin")
    for binary in listing.split():
        status, output = machine.execute(f"${tests}/bin/{binary} --test-threads=1 2>&1")
        with open(machine.out_dir / f"{binary}.txt", "w") as report:
            report.write(f"exit={status}\n{output}")
  '';
}
