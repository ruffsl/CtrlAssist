//! Binary entry point. The code lives in [`ctrlassist_gui`] (the library
//! target) so the integration tests in `tests/` can import the same
//! modules and Slint types the binary uses.

fn main() -> anyhow::Result<()> {
    ctrlassist_gui::run()
}
