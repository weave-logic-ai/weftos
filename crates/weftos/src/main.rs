//! WeftOS daemon -- boots the kernel in any project directory.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "weftos", about = "WeftOS: AI kernel for any project")]
enum Cli {
    /// Initialize WeftOS in the current directory
    Init {
        /// Project directory (default: current)
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Force reinitialize
        #[arg(long)]
        force: bool,
    },
    /// Boot the WeftOS kernel
    Boot {
        /// Project directory
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Show WeftOS status
    Status,
    /// Show version
    Version,
}

#[tokio::main]
async fn main() {
    // Logs go to stderr so stdout stays command output.
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let cli = Cli::parse();

    match cli {
        Cli::Init { path, force } => {
            if weftos::is_initialized(&path) && !force {
                eprintln!(
                    "WeftOS already initialized in {}. Use --force to reinitialize.",
                    path.display()
                );
                std::process::exit(1);
            }
            match weftos::init::init_project(&path) {
                Ok(result) => {
                    println!("WeftOS initialized in {}", result.project_root.display());
                    if result.weave_toml_created {
                        println!("  Created weave.toml");
                    }
                    if result.weftos_dir_created {
                        println!("  Created .weftos/ directory");
                    }
                    println!("\nNext: weftos boot");
                }
                Err(e) => {
                    eprintln!("Init failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Cli::Boot { path } => {
            if !weftos::is_initialized(&path) {
                eprintln!("WeftOS not initialized. Run: weftos init");
                std::process::exit(1);
            }
            println!("Booting WeftOS in {}...", path.display());
            match weftos::WeftOs::boot_in(&path).await {
                Ok(os) => {
                    println!("WeftOS running");
                    println!("  State: {:?}", os.state());
                    println!("  Services: {}", os.service_count());
                    println!("  Processes: {}", os.process_count());
                    println!("\nPress Ctrl+C to stop.");
                    tokio::signal::ctrl_c().await.ok();
                    println!("\nShutting down...");
                    if let Err(e) = os.shutdown().await {
                        eprintln!("Shutdown error: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("Boot failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Cli::Status => {
            if weftos::is_initialized(".") {
                println!("WeftOS initialized in current directory");
                if std::path::Path::new("weave.toml").exists() {
                    println!("  Config: weave.toml");
                }
                if std::path::Path::new(".weftos").exists() {
                    println!("  Runtime: .weftos/");
                }
            } else {
                println!("WeftOS not initialized. Run: weftos init");
            }
        }
        Cli::Version => {
            println!("weftos {}", weftos::VERSION);
        }
    }
}
