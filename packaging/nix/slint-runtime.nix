# System libraries the settings window needs at run time beyond what
# the linker records. Slint's femtovg renderer and winit's Wayland
# backend dlopen most of them, which patchelf can't see, so package.nix
# splices them onto the GUI binary's rpath in postFixup, and flake.nix's
# dev shell exports them via LD_LIBRARY_PATH so `cargo run` works
# in-tree.
#
#   - fontconfig: linked, not dlopened (yeslogic-fontconfig-sys, through
#     fontique), so the build needs it too: package.nix also takes this
#     list as build inputs
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
