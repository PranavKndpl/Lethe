fn main() {
    // Only link specifically for Windows
    #[cfg(target_os = "windows")]
    {
        // Tell Rust to look for .lib files in the current directory
        println!("cargo:rustc-link-search=native=.");
        // Tell Rust to link winfsp-x64.lib
        println!("cargo:rustc-link-lib=winfsp-x64");
    }
}