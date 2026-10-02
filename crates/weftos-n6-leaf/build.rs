use std::{env, fs, path::PathBuf};

fn main() {
    // cortex-m-rt's link.x includes memory.x from the linker search path.
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
}
