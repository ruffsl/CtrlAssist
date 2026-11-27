# The tray's pictures for the user guide, made in a virtual machine: a
# Plasma session for one user, CtrlAssist as its NixOS module installs it,
# the test crate's fake pads, and a profile running on them. What the
# desktop draws of the tray is captured from the machine's screen, so the
# pictures are a real panel's and nobody's own.
#
# The result holds the pictures; `just docs-tray-shots` copies them into
# docs/screenshots.
{
  pkgs,
  fixture,
  package,
  module,
}:
let
  width = 2560;
  height = 1600;
  key = "#ff00ff";
  # The pictures' size, measured from the screen's bottom right corner.
  crop = "1440x1016";

  scene = pkgs.testers.runNixOSTest {
    name = "ctrlassist-tray-shots";

    nodes.machine = {
      imports = [ module ];
      services.ctrlassist = {
        enable = true;
        inherit package;
      };
      # The pads are the test crate's fake ones, which a daemon takes for
      # pads only when asked to.
      systemd.user.services.ctrlassist-daemon.environment.CTRLASSIST_TEST_PADS = "1";

      users.users.alice = {
        isNormalUser = true;
        uid = 1000;
        password = "alice";
      };

      # A Plasma session for alice, logged in as the machine starts.
      services.desktopManager.plasma6.enable = true;
      services.displayManager = {
        sddm.enable = true;
        sddm.wayland.enable = true;
        defaultSession = "plasma";
        autoLogin = {
          enable = true;
          user = "alice";
        };
      };

      virtualisation = {
        memorySize = 4096;
        cores = 4;
        qemu.options = [
          # A screen of twice the pictures' size, which the session draws
          # at a scale of two.
          "-vga none -device virtio-gpu-pci,xres=${toString width},yres=${toString height}"
          # A pointer that is put at a place on the screen, not moved by
          # an amount.
          "-device virtio-tablet-pci"
        ];
      };

      # Nothing on the desktop is seen through: what is behind a tip or a
      # menu in the pictures is the key color.
      environment.etc."xdg/kwinrc".text = ''
        [Plugins]
        blurEnabled=false
        contrastEnabled=false
      '';

      environment.systemPackages = [ fixture ];
    };

    # The script reads the screen to find the tray's icon on it.
    extraPythonPackages = p: [ p.pillow ];
    skipTypeCheck = true;

    testScript = ''
      import shlex
      from PIL import Image

      ENV = "XDG_RUNTIME_DIR=/run/user/1000"
      WIDTH, HEIGHT = ${toString width}, ${toString height}
      # The wallpaper is one color nothing else on the screen has, which
      # the pictures are cut out of afterwards.
      KEY = "${key}"
      # The green of the badge a running profile puts on the tray's icon.
      RUNNING = (0x2E, 0xB8, 0x5C)

      def as_alice(command):
          return "su -l alice -c " + shlex.quote(ENV + " " + command)

      def cli(*arguments):
          return machine.succeed(as_alice("ctrlassist " + shlex.join(arguments)))

      def in_session(*arguments):
          # Run by the session's own service manager, so with the session's
          # display and bus.
          return machine.succeed(
              as_alice("systemd-run --user --wait --collect --quiet -- " + shlex.join(arguments))
          )

      def plasma(script):
          machine.succeed(as_alice(
              "busctl --user call org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell"
              " evaluateScript s " + shlex.quote(script)
          ))

      def send(*events):
          machine.qmp_client.send("input-send-event", {"events": list(events)})

      def point(x, y):
          send(
              {"type": "abs", "data": {"axis": "x", "value": x * 32767 // (WIDTH - 1)}},
              {"type": "abs", "data": {"axis": "y", "value": y * 32767 // (HEIGHT - 1)}},
          )

      def click(button):
          for down in (True, False):
              send({"type": "btn", "data": {"down": down, "button": button}})

      def badge(picture):
          # Where the running badge is on the screen: the middle of the
          # pixels of its color in the panel.
          image = Image.open(machine.out_dir / (picture + ".png")).convert("RGB")
          found = [
              (x, y)
              for y in range(HEIGHT - 120, HEIGHT)
              for x in range(WIDTH)
              if all(abs(a - b) < 24 for a, b in zip(image.getpixel((x, y)), RUNNING))
          ]
          assert len(found) > 20, f"no running badge in the panel of {picture}"
          return (
              sum(x for x, _ in found) // len(found),
              sum(y for _, y in found) // len(found),
          )

      def edge(picture, x, y):
          # The highest row above (x, y) that isn't the wallpaper.
          image = Image.open(machine.out_dir / (picture + ".png")).convert("RGB")
          wallpaper = tuple(bytes.fromhex(KEY[1:]))
          while y > 0 and image.getpixel((x, y - 1)) != wallpaper:
              y -= 1
          return y

      machine.wait_for_unit("multi-user.target")
      # The session, its panel, and the tray in it.
      machine.wait_until_succeeds("pgrep -u alice plasmashell")
      machine.wait_until_succeeds(as_alice("systemctl --user is-active ctrlassist-tray"))
      machine.sleep(20)

      # The desktop the pictures are of: Breeze Dark at a scale of two, a
      # panel that is opaque and against the screen's edge, and the key
      # color for a wallpaper.
      in_session("kscreen-doctor", "output.1.scale.2")
      in_session("plasma-apply-lookandfeel", "-a", "org.kde.breezedark.desktop")
      plasma(
          "for (const desktop of desktops()) {"
          ' desktop.wallpaperPlugin = "org.kde.color";'
          ' desktop.currentConfigGroup = ["Wallpaper", "org.kde.color", "General"];'
          f' desktop.writeConfig("Color", "{KEY}");'
          " }"
          ' for (const panel of panels()) { panel.floating = false; panel.opacity = "opaque"; }'
      )

      # The tray says nothing in notifications, which would be in the
      # pictures of its menu.
      machine.succeed(as_alice(
          "mkdir -p ~/.config/ctrlassist && printf '%s\\n' 'schema = 1'"
          " 'setup_started = false' 'setup_trouble = false'"
          " 'pad_connection = false' 'refused = false' > ~/.config/ctrlassist/tray.toml"
      ))
      machine.succeed(as_alice("systemctl --user restart ctrlassist-tray"))

      # Two fake pads, and the default profile running on them.
      machine.succeed("systemd-run --unit=fake-assist ctrlassist-fixture fake-dualsense --real")
      machine.succeed(
          "systemd-run --unit=fake-primary ctrlassist-fixture fake-pad"
          " --name 'Xbox Wireless Controller' --vendor 045e --product 0b13"
      )
      machine.succeed("udevadm settle")
      machine.succeed(
          "printf '%s\\n' 'mode = \"priority\"' 'primary = \"model:045e:0b13\"'"
          " 'assist = \"02:ca:7e:00:00:01\"' 'identity = \"dualsense\"' > /tmp/default.toml"
      )
      cli("profile", "put", "/tmp/default.toml")
      # A second profile, kept and not running, on two more pads of its
      # own: one that could start has a line in the tip, and its pads
      # are free only if no setup has them.
      machine.succeed(
          "systemd-run --unit=fake-couch-primary ctrlassist-fixture fake-pad"
          " --name 'Nintendo Switch Pro Controller' --vendor 057e --product 2009"
      )
      machine.succeed(
          "systemd-run --unit=fake-couch-assist ctrlassist-fixture fake-pad"
          " --name '8BitDo Pro 2' --vendor 2dc8 --product 6003"
      )
      machine.succeed("udevadm settle")
      machine.succeed(
          "printf '%s\\n' 'mode = \"toggle\"' 'primary = \"model:057e:2009\"'"
          " 'assist = \"model:2dc8:6003\"' > /tmp/couch.toml"
      )
      cli("profile", "put", "/tmp/couch.toml")
      machine.wait_until_succeeds(as_alice("ctrlassist start default"))
      machine.sleep(5)
      machine.screenshot("tray_running")
      x, y = badge("tray_running")

      # The pointer rests on the icon, and the desktop shows the tip.
      point(x - 2, y - 2)
      point(x, y)
      machine.sleep(3)
      machine.screenshot("tray_hover")

      # The menu, and the running profile's own rows beside it: the
      # pointer clicks the menu's first row, which is found above the
      # icon where the key color ends.
      click("right")
      machine.sleep(2)
      machine.screenshot("scene_menu")
      top = edge("scene_menu", x, y)
      point(x, top + 58)
      point(x, top + 60)
      click("left")
      machine.sleep(3)
      machine.screenshot("tray_menu")
      # A click on the empty desktop closes it.
      point(WIDTH // 2, HEIGHT // 2)
      click("left")
      machine.sleep(2)

      # The assist's pad leaves, and the setup is held for it.
      machine.succeed("systemctl stop fake-assist")
      machine.wait_until_succeeds(as_alice("ctrlassist setups --json") + " | grep -q '\"phase\":\"held\"'")
      machine.sleep(3)
      machine.screenshot("tray_held")

      # The daemon stops.
      machine.succeed(as_alice("systemctl --user stop ctrlassist-daemon"))
      machine.sleep(3)
      machine.screenshot("tray_no_daemon")
    '';
  };
in
# Each picture is cut to the corner the tray is in, and the wallpaper is
# taken out of it. How much of the key color a pixel holds is how far its
# red and blue both stand above its green, which is all of them for the
# wallpaper and some for what it shows through: a shadow, the soft edge
# of a corner, a tip the desktop draws see-through. That much is taken
# out. What was mostly the wallpaper is left as see-through as that; what
# was mostly something else is left solid, as it is over a black desktop.
pkgs.runCommand "ctrlassist-tray-shots"
  {
    nativeBuildInputs = [ pkgs.imagemagick ];
    passthru = { inherit scene; };
    keyed = "(min(u.r,u.b)-u.g>0.02 && abs(u.r-u.b)<0.1)";
    key = "(min(u.r,u.b)-u.g)";
  }
  ''
    mkdir -p $out
    solid="($keyed ? 1-$key : 1)"
    for picture in ${scene}/tray_*.png; do
      magick "$picture" -gravity SouthEast -crop ${crop}+0+0 +repage \
        \( +clone -fx "$solid>=0.5 ? 1 : $solid" \) \
        \( -clone 0 -channel RB -fx "$keyed ? (u-$key)/($solid>=0.5 ? 1 : $solid+0.0001) : u" \
          -channel G -fx "$solid>=0.5 ? u : u/($solid+0.0001)" +channel \) \
        -delete 0 +swap -alpha off -compose CopyOpacity -composite \
        "$out/system_$(basename "$picture")"
    done
  ''
