//! `clawft-lsp-extract` — LSP-based code intelligence extraction.
//!
//! Taps Language Server Protocol servers to extract full semantic graphs
//! from any supported language. Captures the "digital exhaust" that IDEs
//! already compute: symbols, references, call hierarchies, type relationships,
//! and dependency structures.
//!
//! # Supported Languages
//!
//! Any language with an LSP server. Built-in configurations for:
//! - **Rust** via `rust-analyzer`
//! - **TypeScript/JavaScript** via `typescript-language-server`
//! - **Python** via `pylsp` or `pyright`
//! - **Go** via `gopls`
//!
//! # Architecture
//!
//! ```text
//! Source code → LSP server (subprocess) → JSON-RPC → Extraction
//!   ↓
//! Symbols (textDocument/documentSymbol)
//!   + References (textDocument/references)
//!   + Call hierarchy (callHierarchy/incomingCalls, outgoingCalls)
//!   + Type hierarchy (typeHierarchy/supertypes, subtypes)
//!   ↓
//! LspGraph { nodes: Vec<LspNode>, edges: Vec<LspEdge> }
//! ```

pub mod config;
pub mod extract;
pub mod graph;
pub mod protocol;
pub mod server;

pub use config::LanguageConfig;
pub use graph::{LspEdge, LspEdgeKind, LspGraph, LspNode, LspNodeKind};
