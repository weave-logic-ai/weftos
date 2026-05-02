//! `weaver update` / `weft update` — Self-update both binaries.
//!
//! Downloads the latest release from GitHub Releases and replaces
//! both `weft` and `weaver` binaries in-place.

use std::path::PathBuf;

use clap::Subcommand;

const REPO: &str = "weave-logic-ai/weftos";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Update subcommands.
#[derive(Debug, Subcommand)]
pub enum UpdateCmd {
    /// Check for new versions without installing.
    Check,
    /// Download and install the latest version of both binaries.
    Install {
        /// Force reinstall even if already on latest.
        #[arg(long)]
        force: bool,
    },
}

impl Default for UpdateCmd {
    fn default() -> Self {
        Self::Install { force: false }
    }
}

pub async fn run(cmd: UpdateCmd) -> anyhow::Result<()> {
    match cmd {
        UpdateCmd::Check => check().await,
        UpdateCmd::Install { force } => install(force).await,
    }
}

/// Just run install with no subcommand (`weaver update` = `weaver update install`).
pub async fn run_default() -> anyhow::Result<()> {
    install(false).await
}

async fn check() -> anyhow::Result<()> {
    let latest = fetch_latest_version().await?;
    println!("Current: v{CURRENT_VERSION}");
    println!("Latest:  v{latest}");
    if latest == CURRENT_VERSION {
        println!("You are up to date.");
    } else {
        println!("Update available. Run: weaver update install");
    }
    Ok(())
}

async fn install(force: bool) -> anyhow::Result<()> {
    let latest = fetch_latest_version().await?;
    println!("Current: v{CURRENT_VERSION}");
    println!("Latest:  v{latest}");

    if latest == CURRENT_VERSION && !force {
        println!("Already on latest. Use --force to reinstall.");
        return Ok(());
    }

    let triple = detect_target_triple();
    println!("Platform: {triple}");
    println!();

    // Download both binaries
    let bins = [
        ("clawft-cli", "weft"),
        ("clawft-weave", "weaver"),
        ("weftos", "weftos"),
    ];

    let temp_dir = tempfile::tempdir()?;

    for (asset_prefix, bin_name) in &bins {
        let asset = format!("{asset_prefix}-{triple}.tar.gz");
        let url = format!(
            "https://github.com/{REPO}/releases/download/v{latest}/{asset}"
        );

        println!("Downloading {bin_name}...");
        let tarball_path = temp_dir.path().join(&asset);

        // Download with curl (available everywhere)
        let status = std::process::Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&tarball_path)
            .arg(&url)
            .status()?;

        if !status.success() {
            eprintln!("  Skipping {bin_name} — asset not found: {asset}");
            continue;
        }

        // Extract
        let extract_dir = temp_dir.path().join(bin_name);
        std::fs::create_dir_all(&extract_dir)?;

        let status = std::process::Command::new("tar")
            .args(["xzf"])
            .arg(&tarball_path)
            .arg("--strip-components=1")
            .arg("-C")
            .arg(&extract_dir)
            .status()?;

        if !status.success() {
            anyhow::bail!("failed to extract {asset}");
        }

        // Find the binary in extracted dir
        let extracted_bin = extract_dir.join(bin_name);
        if !extracted_bin.exists() {
            eprintln!("  Skipping {bin_name} — binary not found in archive");
            continue;
        }

        // Find where the current binary lives
        let install_path = find_install_path(bin_name)?;
        println!("  Installing to: {}", install_path.display());

        // Replace binary
        replace_binary(&extracted_bin, &install_path)?;
        println!("  ✓ {bin_name} updated to v{latest}");
    }

    println!();
    println!("Update complete. Both binaries are now v{latest}.");
    println!();
    println!("If the kernel is running, restart it:");
    println!("  weaver kernel stop && weaver kernel start");

    Ok(())
}

fn detect_target_triple() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        // Check if musl or glibc
        if is_musl() {
            "x86_64-unknown-linux-musl"
        } else {
            "x86_64-unknown-linux-gnu"
        }
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        if is_musl() {
            "aarch64-unknown-linux-musl"
        } else {
            "aarch64-unknown-linux-gnu"
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    { "x86_64-apple-darwin" }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    { "aarch64-apple-darwin" }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    { "x86_64-pc-windows-msvc" }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    { "unknown" }
}

#[cfg(target_os = "linux")]
fn is_musl() -> bool {
    // Check if current binary is statically linked (musl)
    std::process::Command::new("ldd")
        .arg(std::env::current_exe().unwrap_or_default())
        .output()
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            out.contains("musl") || !o.status.success()
        })
        .unwrap_or(false)
}

fn find_install_path(bin_name: &str) -> anyhow::Result<PathBuf> {
    // 1. Check where the current binary lives
    if bin_name == "weaver"
        && let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent() {
                return Ok(dir.join(bin_name));
            }

    // 2. Check PATH for existing installation
    let which = std::process::Command::new("which")
        .arg(bin_name)
        .output();
    if let Ok(output) = which
        && output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }

    // 3. Default to /usr/local/bin
    Ok(PathBuf::from("/usr/local/bin").join(bin_name))
}

fn replace_binary(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(src)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(src, perms)?;
    }

    // On Linux, a running binary can't be overwritten (ETXTBSY / "Text file
    // busy"). The fix: rename the old binary out of the way first — rename()
    // works on busy executables because it operates on the directory entry,
    // not the file. The kernel keeps the old inode alive until the process
    // exits, but the path is now free for the new binary.
    let backup = dst.with_extension("old");

    // Try rename-then-copy (handles "Text file busy").
    if dst.exists()
        && std::fs::rename(dst, &backup).is_ok() {
            match std::fs::copy(src, dst) {
                Ok(_) => {
                    let _ = std::fs::remove_file(&backup);
                    return Ok(());
                }
                Err(e) => {
                    // Restore backup if copy fails.
                    let _ = std::fs::rename(&backup, dst);
                    eprintln!("  Copy failed after rename: {e}");
                }
            }
        }

    // Try direct copy (works when binary isn't running).
    if std::fs::copy(src, dst).is_ok() {
        return Ok(());
    }

    // Last resort: sudo cp.
    eprintln!("  Permission denied, trying with sudo...");
    let status = std::process::Command::new("sudo")
        .args(["cp"])
        .arg(src)
        .arg(dst)
        .status()?;

    if !status.success() {
        anyhow::bail!(
            "failed to install to {} — try: sudo cp {} {}",
            dst.display(),
            src.display(),
            dst.display()
        );
    }

    Ok(())
}

async fn fetch_latest_version() -> anyhow::Result<String> {
    // Use GitHub API to get latest release tag
    let output = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "-H", "Accept: application/vnd.github.v3+json",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        ])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("failed to fetch latest release from GitHub");
    }

    let body: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let tag = body["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no tag_name in release response"))?;

    // Strip leading 'v'
    Ok(tag.strip_prefix('v').unwrap_or(tag).to_string())
}
