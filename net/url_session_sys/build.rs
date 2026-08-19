fn main() {
    // Only compile the URLSession wrapper on Apple targets.
    #[cfg(target_vendor = "apple")]
    {
        cc::Build::new()
            .file("src/url_session_wrapper.m")
            .flag("-fblocks")
            .compile("url_session_wrapper");

        println!("cargo:rustc-link-lib=framework=Foundation");
    }

    println!("cargo:rerun-if-changed=src/url_session_wrapper.m");
    println!("cargo:rerun-if-changed=src/url_session_wrapper.h");
}
