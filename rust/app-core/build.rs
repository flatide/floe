fn main() {
    // The native indexer owns both identities. Do not maintain independent
    // magic numbers in the new application shell.
    let manifest = "../cli/Cargo.toml";
    let source = "../cli/src/main.rs";
    println!("cargo:rerun-if-changed={manifest}");
    println!("cargo:rerun-if-changed={source}");
    let text = std::fs::read_to_string(manifest).expect("indexer manifest");
    let version = text
        .lines()
        .find_map(|l| l.strip_prefix("version = \"")?.strip_suffix('"'))
        .expect("indexer version");
    println!("cargo:rustc-env=FLOE_INDEX_VERSION={version}");
    let text = std::fs::read_to_string(source).expect("indexer source");
    let version = text
        .lines()
        .find_map(|l| {
            l.strip_prefix("const CACHE_VERSION: u64 = ")?
                .strip_suffix(';')
        })
        .expect("cache version");
    println!("cargo:rustc-env=FLOE_CACHE_VERSION={version}");
}
