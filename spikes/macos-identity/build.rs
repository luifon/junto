fn main() {
    println!("cargo:rerun-if-env-changed=SPIKE_VERSION");
    let version = std::env::var("SPIKE_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    println!("cargo:rustc-env=SPIKE_VERSION={version}");
}
