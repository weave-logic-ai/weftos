//! WASI filesystem stub.
//!
//! Provides a [`WasiFileSystem`] with a self-contained API for filesystem operations.
//! Currently all methods return `Unsupported` errors. Once WASI filesystem APIs
//! (`wasi:filesystem/types` and `wasi:filesystem/preopens`) are stable and
//! accessible from Rust, this will be replaced with real filesystem operations
//! scoped to the pre-opened directories granted by the WASI host.
//!
//! This module is fully decoupled from `clawft-platform` so it can compile for
//! `wasm32-wasip2` without pulling in tokio or reqwest.

use std::path::{Path, PathBuf};

/// Filesystem for WASI environments.
///
/// This is a stub implementation that will use WASI filesystem capabilities
/// (`wasi:filesystem/types`, `wasi:filesystem/preopens`) once they are stable.
/// Until then, all methods return [`std::io::ErrorKind::Unsupported`] errors.
pub struct WasiFileSystem;

impl WasiFileSystem {
    /// Create a new WASI filesystem handle.
    pub fn new() -> Self {
        Self
    }

    /// Read a file's entire contents as a UTF-8 string.
    pub fn read_to_string(&self, _path: &Path) -> std::io::Result<String> {
        Err(unsupported("read_to_string"))
    }

    /// Write a string to a file, creating parent directories if needed.
    pub fn write_string(&self, _path: &Path, _content: &str) -> std::io::Result<()> {
        Err(unsupported("write_string"))
    }

    /// Append a string to a file.
    pub fn append_string(&self, _path: &Path, _content: &str) -> std::io::Result<()> {
        Err(unsupported("append_string"))
    }

    /// Check whether a path exists.
    pub fn exists(&self, _path: &Path) -> bool {
        false
    }

    /// List all entries in a directory.
    pub fn list_dir(&self, _path: &Path) -> std::io::Result<Vec<PathBuf>> {
        Err(unsupported("list_dir"))
    }

    /// Create a directory and all parent directories.
    pub fn create_dir_all(&self, _path: &Path) -> std::io::Result<()> {
        Err(unsupported("create_dir_all"))
    }

    /// Remove a file.
    pub fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
        Err(unsupported("remove_file"))
    }

    /// Get the user's home directory.
    ///
    /// WASI environments have no concept of a user home directory.
    pub fn home_dir(&self) -> Option<PathBuf> {
        None
    }
}

impl Default for WasiFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create an unsupported error with a descriptive message.
fn unsupported(operation: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("WASI filesystem not yet implemented: {operation}"),
    )
}

// ---------------------------------------------------------------------------
// Sandboxed filesystem (behind wasm-plugins feature)
// ---------------------------------------------------------------------------

/// Sandboxed filesystem that validates all operations against a plugin's
/// permissions before executing them.
///
/// Only available when the `wasm-plugins` feature is enabled.
#[cfg(feature = "wasm-plugins")]
pub struct SandboxedFileSystem {
    /// The plugin sandbox that governs all access decisions.
    pub sandbox: std::sync::Arc<crate::sandbox::PluginSandbox>,
}

#[cfg(feature = "wasm-plugins")]
impl SandboxedFileSystem {
    /// Create a new sandboxed filesystem for a plugin.
    pub fn new(sandbox: std::sync::Arc<crate::sandbox::PluginSandbox>) -> Self {
        Self { sandbox }
    }

    /// Read a file's entire contents as a UTF-8 string.
    ///
    /// The path is validated against the plugin's filesystem permissions,
    /// canonicalized, checked for symlink escapes, and the file size is
    /// verified against the 8 MB read limit before reading.
    pub fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        let canonical = crate::sandbox::validate_file_access(&self.sandbox, path, false)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
        std::fs::read_to_string(canonical)
    }

    /// Write a string to a file, creating it if it does not exist.
    ///
    /// The path is validated against the plugin's filesystem permissions.
    /// The parent directory is canonicalized for sandbox containment.
    /// Content size is enforced at 4 MB.
    pub fn write_string(&self, path: &Path, content: &str) -> std::io::Result<()> {
        if content.len() > crate::sandbox::MAX_WRITE_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "write content too large: {} bytes, max {} bytes",
                    content.len(),
                    crate::sandbox::MAX_WRITE_SIZE
                ),
            ));
        }
        let canonical = crate::sandbox::validate_file_access(&self.sandbox, path, true)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
        std::fs::write(canonical, content)
    }

    /// Append a string to a file.
    ///
    /// Validates the path and appends content. The file must already exist
    /// within the sandbox.
    pub fn append_string(&self, path: &Path, content: &str) -> std::io::Result<()> {
        if content.len() > crate::sandbox::MAX_WRITE_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "append content too large: {} bytes, max {} bytes",
                    content.len(),
                    crate::sandbox::MAX_WRITE_SIZE
                ),
            ));
        }
        let canonical = crate::sandbox::validate_file_access(&self.sandbox, path, false)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(canonical)?;
        file.write_all(content.as_bytes())
    }

    /// Check whether a path exists within the sandbox.
    ///
    /// Returns `false` if the path is outside the sandbox or does not exist.
    pub fn exists(&self, path: &Path) -> bool {
        crate::sandbox::validate_file_access(&self.sandbox, path, false).is_ok()
    }

    /// List all entries in a directory within the sandbox.
    pub fn list_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        let canonical = crate::sandbox::validate_file_access(&self.sandbox, path, false)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(canonical)? {
            entries.push(entry?.path());
        }
        Ok(entries)
    }

    /// Create a directory and all parent directories within the sandbox.
    pub fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        let canonical = crate::sandbox::validate_file_access(&self.sandbox, path, true)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
        std::fs::create_dir_all(canonical)
    }

    /// Remove a file within the sandbox.
    pub fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        let canonical = crate::sandbox::validate_file_access(&self.sandbox, path, false)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
        std::fs::remove_file(canonical)
    }

    /// Get the user's home directory.
    ///
    /// Delegates to `dirs::home_dir()` when available.
    pub fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasi_filesystem_can_be_created() {
        let _fs = WasiFileSystem::new();
    }

    #[test]
    fn wasi_filesystem_default() {
        let _fs = WasiFileSystem;
    }

    #[test]
    fn read_to_string_returns_unsupported() {
        let fs = WasiFileSystem::new();
        let result = fs.read_to_string(Path::new("/tmp/test.txt"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(err.to_string().contains("read_to_string"));
    }

    #[test]
    fn write_string_returns_unsupported() {
        let fs = WasiFileSystem::new();
        let result = fs.write_string(Path::new("/tmp/test.txt"), "content");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn append_string_returns_unsupported() {
        let fs = WasiFileSystem::new();
        let result = fs.append_string(Path::new("/tmp/test.txt"), "more");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn exists_returns_false() {
        let fs = WasiFileSystem::new();
        assert!(!fs.exists(Path::new("/tmp/test.txt")));
    }

    #[test]
    fn list_dir_returns_unsupported() {
        let fs = WasiFileSystem::new();
        let result = fs.list_dir(Path::new("/tmp"));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn create_dir_all_returns_unsupported() {
        let fs = WasiFileSystem::new();
        let result = fs.create_dir_all(Path::new("/tmp/a/b/c"));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn remove_file_returns_unsupported() {
        let fs = WasiFileSystem::new();
        let result = fs.remove_file(Path::new("/tmp/test.txt"));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn home_dir_returns_none() {
        let fs = WasiFileSystem::new();
        assert!(fs.home_dir().is_none());
    }

    #[test]
    fn wasi_filesystem_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WasiFileSystem>();
    }

    // -- SandboxedFileSystem tests (wasm-plugins feature) --

    #[cfg(feature = "wasm-plugins")]
    mod sandboxed {
        use super::*;
        use crate::sandbox::PluginSandbox;
        use clawft_plugin::{PluginPermissions, PluginResourceConfig};
        use std::sync::Arc;

        fn sandbox_with_dir(dir: &Path) -> Arc<PluginSandbox> {
            let permissions = PluginPermissions {
                filesystem: vec![dir.to_string_lossy().to_string()],
                ..Default::default()
            };
            Arc::new(PluginSandbox::from_manifest(
                "test-plugin".into(),
                permissions,
                &PluginResourceConfig::default(),
            ))
        }

        fn sandbox_no_fs() -> Arc<PluginSandbox> {
            Arc::new(PluginSandbox::from_manifest(
                "test-plugin".into(),
                PluginPermissions::default(),
                &PluginResourceConfig::default(),
            ))
        }

        #[test]
        fn sandboxed_read_within_allowed_path() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_read");
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join("test.txt");
            std::fs::write(&file, "hello sandbox").unwrap();

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            let result = fs.read_to_string(&file);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "hello sandbox");

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_read_outside_sandbox_denied() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_read_deny");
            let _ = std::fs::create_dir_all(&dir);

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            let result = fs.read_to_string(Path::new("/etc/hosts"));
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_write_within_allowed_path() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_write");
            let _ = std::fs::create_dir_all(&dir);

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            let file = dir.join("output.txt");
            let result = fs.write_string(&file, "written");
            assert!(result.is_ok());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "written");

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_write_outside_sandbox_denied() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_write_deny");
            let _ = std::fs::create_dir_all(&dir);

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            let result = fs.write_string(Path::new("/etc/hacked.txt"), "bad");
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_write_too_large_rejected() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_write_large");
            let _ = std::fs::create_dir_all(&dir);

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            let large = "x".repeat(5 * 1024 * 1024); // 5 MB > 4 MB limit
            let file = dir.join("big.txt");
            let result = fs.write_string(&file, &large);
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_append_within_sandbox() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_append");
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join("log.txt");
            std::fs::write(&file, "line1\n").unwrap();

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            let result = fs.append_string(&file, "line2\n");
            assert!(result.is_ok());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "line1\nline2\n");

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_exists_within_sandbox() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_exists");
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join("exists.txt");
            std::fs::write(&file, "yes").unwrap();

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            assert!(fs.exists(&file));
            assert!(!fs.exists(&dir.join("nope.txt")));

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_exists_outside_sandbox_returns_false() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_exists_deny");
            let _ = std::fs::create_dir_all(&dir);

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            assert!(!fs.exists(Path::new("/etc/hosts")));

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_list_dir_within_sandbox() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_list");
            let _ = std::fs::create_dir_all(&dir);
            std::fs::write(dir.join("a.txt"), "a").unwrap();
            std::fs::write(dir.join("b.txt"), "b").unwrap();

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            let result = fs.list_dir(&dir);
            assert!(result.is_ok());
            let entries = result.unwrap();
            assert_eq!(entries.len(), 2);

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_create_dir_within_sandbox() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_mkdir");
            let _ = std::fs::create_dir_all(&dir);

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            let sub = dir.join("sub_dir");
            let result = fs.create_dir_all(&sub);
            assert!(result.is_ok());
            assert!(sub.is_dir());

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_remove_file_within_sandbox() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_rm");
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join("deleteme.txt");
            std::fs::write(&file, "bye").unwrap();

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            let result = fs.remove_file(&file);
            assert!(result.is_ok());
            assert!(!file.exists());

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sandboxed_no_fs_permissions_denied() {
            let fs = SandboxedFileSystem::new(sandbox_no_fs());
            let result = fs.read_to_string(Path::new("/tmp/test.txt"));
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
        }

        #[test]
        fn sandboxed_home_dir_returns_some() {
            let dir = std::env::temp_dir().join("clawft_sandboxed_fs_home");
            let _ = std::fs::create_dir_all(&dir);

            let fs = SandboxedFileSystem::new(sandbox_with_dir(&dir));
            // home_dir delegates to dirs::home_dir() which should return Some on Linux
            let home = fs.home_dir();
            assert!(home.is_some());

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
