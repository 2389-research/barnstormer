// ABOUTME: Build script for the barnstormer-tauri desktop crate.
// ABOUTME: Delegates to tauri-build so codegen and resource bundling run at compile time.

fn main() {
    tauri_build::build()
}
