fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Tell rustc about the custom cfg so it doesn't warn.
    println!("cargo::rustc-check-cfg=cfg(url_session_default)");

    let target_vendor = std::env::var("CARGO_CFG_TARGET_VENDOR").unwrap_or_default();
    let is_apple = target_vendor == "apple";

    // Detect whether the user explicitly selected a backend feature.
    let tokio_enabled = std::env::var("CARGO_FEATURE_TOKIO").is_ok();
    let url_session_enabled = std::env::var("CARGO_FEATURE_URL_SESSION").is_ok();

    // When no backend feature is explicitly selected, pick the platform
    // default: URLSession on Apple, tokio elsewhere (reqwest is always
    // compiled on non-Apple platforms, see Cargo.toml, so it needs no cfg).
    if !tokio_enabled && !url_session_enabled && is_apple {
        println!("cargo:rustc-cfg=url_session_default");
    }
}
