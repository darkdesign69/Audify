fn main() {
    // tauri-build watches tauri.conf.json but not the icons it embeds.
    println!("cargo:rerun-if-changed=icons");

    // Android: link against the libmpv.so unpacked by scripts/build-android.sh into
    // ../libmpv-link. Done here because the Tauri CLI overwrites RUSTFLAGS under Gradle.
    println!("cargo:rerun-if-changed=../libmpv-link");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        let dir = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("../libmpv-link");
        match dir.canonicalize() {
            Ok(dir) => {
                println!("cargo:rustc-link-search=native={}", dir.display());
                println!("cargo:rustc-link-arg-cdylib=-L{}", dir.display());
                println!("cargo:rustc-link-arg-cdylib=-Wl,-rpath-link,{}", dir.display());
                println!("cargo:rustc-link-arg-cdylib=-Wl,--allow-shlib-undefined");
            }
            Err(e) => println!("cargo:warning=libmpv-link not found: {e}"),
        }
    }

    tauri_build::build()
}
