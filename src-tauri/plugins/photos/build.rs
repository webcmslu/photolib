// The plugin has no commands of its own for the interface: the app calls it
// from Rust (src/mobile.rs in the app).
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
