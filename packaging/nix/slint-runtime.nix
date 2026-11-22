# System libraries Slint's femtovg renderer + winit's Wayland backend
# dlopen at runtime. patchelf can't see dlopen targets, so package.nix
# splices them onto the GUI binary's rpath in postFixup, and
# flake.nix's dev shell exports them via LD_LIBRARY_PATH so `cargo
# run` works in-tree.
#
#   - fontconfig: femtovg renderer (default)
#   - libGL:      OpenGL ES backend (winit + glutin EGL dlopen)
#   - libxkbcommon: winit pulls xkbcommon-dl for keymap parsing,
#     even on pure-Wayland sessions
#   - wayland: winit Wayland backend
#
# X11 libs (libxcb, libx11, libxcursor, libxi, libxrandr) are absent
# by design — ctrlassist-gui builds Slint with `backend-winit-wayland`
# only, and the dep graph has no x11rb / x11-dl entries.
{
  fontconfig,
  libGL,
  libxkbcommon,
  wayland,
}:

[
  fontconfig
  libGL
  libxkbcommon
  wayland
]
