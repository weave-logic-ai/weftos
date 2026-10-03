use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    println!("cargo:rerun-if-changed=memory.x");
}
