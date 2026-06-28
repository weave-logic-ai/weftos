// Wires esp-idf-sys into Cargo's build graph: tells Cargo about the
// sysenv vars embuild/idf-sys sets (linker paths, sysroot, includes,
// CFG flags). Same one-liner as crates/clawft-edge-bench.
fn main() {
    embuild::espidf::sysenv::output();
}
