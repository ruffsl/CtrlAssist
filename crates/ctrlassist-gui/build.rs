//! Compiles the Slint UI; `slint::include_modules!()` in `src/lib.rs` reads
//! the generated code.

fn main() {
    // Element ids in the generated code, which `i-slint-backend-testing`'s
    // `find_by_element_id` / `accessible_id()` and the Slint MCP server
    // need; without them those finders silently return nothing. Emitted
    // for every profile but release-derived ones, which ship without the
    // extra code. This overrides the SLINT_EMIT_DEBUG_INFO variable.
    let debug_info = std::env::var("PROFILE").as_deref() != Ok("release");
    let config = slint_build::CompilerConfiguration::new().with_debug_info(debug_info);
    // The project file names the main file and the language edition, for
    // this build and for the editor tooling: the LSP finds it from any
    // `.slint` file under the crate (walking up within the editor's
    // workspace), and the viewer is given the crate directory. A `.slint`
    // path alone compiles without the edition.
    slint_build::compile_with_config("slint-project.json", config)
        .expect("the Slint UI failed to compile; slint-build printed the errors above");
}
