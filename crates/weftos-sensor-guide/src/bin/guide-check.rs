//! `guide-check <guide-dir>...`: validates sensor guides (ADR-104) for a cog's gate.
//! Exit 0 when every guide loads and `validate()` finds nothing; 1 otherwise.

fn main() {
    let dirs: Vec<String> = std::env::args().skip(1).collect();
    if dirs.is_empty() {
        eprintln!("usage: guide-check <guide-dir>...");
        std::process::exit(2);
    }
    let mut failed = false;
    for d in &dirs {
        match weftos_sensor_guide::GuideBundle::from_dir(std::path::Path::new(d)) {
            Ok(b) => {
                let errs = b.validate();
                if errs.is_empty() {
                    println!("ok   {d}: {} ({} pages)", b.doc.title, b.pages.len());
                } else {
                    failed = true;
                    for e in errs {
                        println!("FAIL {d}: {e}");
                    }
                }
            }
            Err(e) => {
                failed = true;
                println!("FAIL {d}: {e}");
            }
        }
    }
    std::process::exit(i32::from(failed));
}
