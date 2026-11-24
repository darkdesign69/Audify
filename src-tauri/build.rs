fn main() {
    // tauri-build watches tauri.conf.json but not the icons it embeds.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
