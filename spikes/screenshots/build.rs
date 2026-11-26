fn main() {
    // The window's release, for the made-up state to agree with About.
    let manifest = std::fs::read_to_string("../../Cargo.toml").expect("the workspace manifest");
    let version = manifest
        .lines()
        .skip_while(|line| line.trim() != "[workspace.package]")
        .find_map(|line| line.strip_prefix("version = "))
        .expect("the workspace's version")
        .trim_matches('"')
        .to_string();
    println!("cargo:rustc-env=CARGO_PKG_VERSION_OF_WINDOW={version}");
}
