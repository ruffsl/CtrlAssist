//! Puts the third-party license bundle where `about.rs` embeds it from,
//! and tells the crate what it was built for.
//!
//! The bundle is written by `just licenses-html` (cargo-about), which the
//! commit hooks and every package build run; it isn't kept in the
//! repository. This script only copies it, so a build of the window
//! needs no tool beyond cargo. A tree where it was never written gets a
//! page that says how to write it, and the build still succeeds.

use std::path::PathBuf;

fn main() {
    let bundle = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/licensing/THIRD-PARTY-LICENSES.html");
    // Named whether or not it exists, so writing it later rebuilds this.
    println!("cargo::rerun-if-changed={}", bundle.display());
    let html = std::fs::read_to_string(&bundle).unwrap_or_else(|_| {
        "<!doctype html><title>Third-party licenses</title>\
         <p>This build of CtrlAssist was made from a source tree in which the \
         third-party license bundle hadn't been written. \
         <code>just licenses-html</code> writes it; a packaged CtrlAssist \
         installs it under <code>share/doc/ctrlassist</code>.</p>"
            .to_string()
    });
    // What the window was built for, for the diagnostic bundle.
    println!(
        "cargo::rustc-env=CTRLASSIST_BUILD_TARGET={}",
        std::env::var("TARGET").expect("cargo sets TARGET for build scripts")
    );
    let out =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for build scripts"))
            .join("THIRD-PARTY-LICENSES.html");
    std::fs::write(&out, html).expect("the build's own output directory can be written");
}
