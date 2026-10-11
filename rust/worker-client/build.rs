fn main() {
    // One compatibility source: do not confuse the product version with the
    // daemon wire version or maintain a third independent version constant.
    let manifest = "../renderd/Cargo.toml";
    println!("cargo:rerun-if-changed={manifest}");
    let text = std::fs::read_to_string(manifest).expect("renderd manifest");
    let version = text
        .lines()
        .find_map(|line| line.strip_prefix("version = \"")?.strip_suffix('"'))
        .expect("renderd package version");
    println!("cargo:rustc-env=FLOE_RENDERD_VERSION={version}");
}
