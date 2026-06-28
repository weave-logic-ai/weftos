//! Tool implementations for clawft.
//!
//! Provides file, shell, and memory tools that implement the
//! `Tool` trait from clawft-core.
//!
//! # Tools
//!
//! - **File tools** ([`file_tools`]): `read_file`, `write_file`, `edit_file`, `list_directory`
//! - **Shell tool** ([`shell_tool`]): `exec_shell`
//! - **Memory tools** ([`memory_tool`]): `memory_read`, `memory_write`
//!
//! All file and directory operations enforce workspace path containment
//! to prevent directory traversal attacks.

#[cfg(feature = "voice")]
pub mod audio_synthesize;
#[cfg(feature = "voice")]
pub mod audio_transcribe;
#[cfg(feature = "delegate")]
pub mod delegate_tool;
pub mod file_tools;
pub mod memory_tool;
pub mod message_tool;
#[cfg(feature = "canvas")]
pub mod render_ui;
pub mod security_policy;
#[cfg(feature = "native-exec")]
pub mod shell_tool;
#[cfg(feature = "native-exec")]
pub mod spawn_tool;
pub mod url_safety;
#[cfg(feature = "voice")]
pub mod voice_listen;
#[cfg(feature = "voice")]
pub mod voice_speak;
pub mod web_fetch;
pub mod web_search;

use std::path::PathBuf;
use std::sync::Arc;

use clawft_core::tools::registry::ToolRegistry;
use clawft_platform::Platform;

use crate::security_policy::CommandPolicy;
use crate::url_safety::UrlPolicy;
use crate::web_search::WebSearchConfig;

/// Register all built-in tools with the given registry.
///
/// Creates instances of every tool in this crate and registers them
/// with `registry`. File tools are sandboxed to `workspace_dir`.
/// Shell and spawn tools are gated by `command_policy` and require the
/// `native-exec` feature. Web fetch tool is gated by `url_policy` for
/// SSRF protection.
///
/// # Arguments
///
/// * `registry` - The tool registry to populate.
/// * `platform` - Platform abstraction for filesystem and process operations.
/// * `workspace_dir` - Root directory that file tools are restricted to.
/// * `command_policy` - Security policy for shell/spawn command execution.
/// * `url_policy` - Security policy for URL fetching (SSRF protection).
/// * `web_search_config` - Configuration for the web search tool (API key / endpoint).
pub fn register_all<P: Platform + 'static>(
    registry: &mut ToolRegistry,
    platform: Arc<P>,
    workspace_dir: PathBuf,
    command_policy: CommandPolicy,
    url_policy: UrlPolicy,
    web_search_config: WebSearchConfig,
) {
    // Suppress unused warning when native-exec is disabled.
    #[cfg(not(feature = "native-exec"))]
    let _ = &command_policy;

    registry.register(Arc::new(file_tools::ReadFileTool::new(
        platform.clone(),
        workspace_dir.clone(),
    )));
    registry.register(Arc::new(file_tools::WriteFileTool::new(
        platform.clone(),
        workspace_dir.clone(),
    )));
    registry.register(Arc::new(file_tools::EditFileTool::new(
        platform.clone(),
        workspace_dir.clone(),
    )));
    registry.register(Arc::new(file_tools::ListDirectoryTool::new(
        platform.clone(),
        workspace_dir.clone(),
    )));

    #[cfg(feature = "native-exec")]
    registry.register(Arc::new(shell_tool::ShellExecTool::new(
        workspace_dir.clone(),
        command_policy.clone(),
    )));

    registry.register(Arc::new(memory_tool::MemoryReadTool::new(platform.clone())));
    registry.register(Arc::new(memory_tool::MemoryWriteTool::new(
        platform.clone(),
    )));
    registry.register(Arc::new(web_search::WebSearchTool::new(
        platform.clone(),
        web_search_config,
    )));
    registry.register(Arc::new(web_fetch::WebFetchTool::new(
        platform.clone(),
        url_policy,
    )));

    #[cfg(feature = "native-exec")]
    registry.register(Arc::new(spawn_tool::SpawnTool::new(
        platform,
        workspace_dir,
        command_policy,
    )));

    #[cfg(feature = "voice")]
    {
        registry.register(Arc::new(voice_listen::VoiceListenTool::new()));
        registry.register(Arc::new(voice_speak::VoiceSpeakTool::new()));
        registry.register(Arc::new(audio_transcribe::AudioTranscribeTool::new()));
        registry.register(Arc::new(audio_synthesize::AudioSynthesizeTool::new()));
    }

    #[cfg(feature = "canvas")]
    registry.register(Arc::new(render_ui::RenderUiTool::new()));
}
