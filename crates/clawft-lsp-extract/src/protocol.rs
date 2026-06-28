//! LSP JSON-RPC protocol implementation.
//!
//! Handles the wire format for communicating with language servers
//! over stdin/stdout: Content-Length headers + JSON-RPC 2.0 messages.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

/// A JSON-RPC request to the language server.
#[derive(Debug, Serialize)]
pub struct RpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    pub params: serde_json::Value,
}

/// A JSON-RPC response from the language server.
#[derive(Debug, Deserialize)]
pub struct RpcResponse {
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// A JSON-RPC notification (no id, no response expected).
#[derive(Debug, Serialize)]
pub struct RpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: serde_json::Value,
}

/// Write an LSP message with Content-Length header.
pub fn write_message<W: Write>(writer: &mut W, content: &[u8]) -> std::io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", content.len())?;
    writer.write_all(content)?;
    writer.flush()
}

/// Read an LSP message by parsing Content-Length header.
pub fn read_message<R: BufRead>(reader: &mut R) -> std::io::Result<String> {
    let mut content_length: usize = 0;

    // Read headers.
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        let header = header.trim();

        if header.is_empty() {
            break;
        }

        if let Some(len_str) = header.strip_prefix("Content-Length: ") {
            content_length = len_str.parse().unwrap_or(0);
        }
    }

    if content_length == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing or zero Content-Length",
        ));
    }

    // Read body.
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    String::from_utf8(body).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Build an initialize request for the LSP handshake.
pub fn initialize_request(id: u64, root_uri: &str, init_options: &serde_json::Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0",
        id,
        method: "initialize".into(),
        params: serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "documentSymbol": {
                        "hierarchicalDocumentSymbolSupport": true,
                    },
                    "references": {},
                    "callHierarchy": {
                        "dynamicRegistration": false,
                    },
                    "typeHierarchy": {
                        "dynamicRegistration": false,
                    },
                },
                "workspace": {
                    "symbol": {
                        "dynamicRegistration": false,
                    },
                },
            },
            "initializationOptions": init_options,
        }),
    }
}

/// Build an initialized notification (sent after initialize response).
pub fn initialized_notification() -> RpcNotification {
    RpcNotification {
        jsonrpc: "2.0",
        method: "initialized".into(),
        params: serde_json::json!({}),
    }
}

/// Build a textDocument/documentSymbol request.
pub fn document_symbol_request(id: u64, uri: &str) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0",
        id,
        method: "textDocument/documentSymbol".into(),
        params: serde_json::json!({
            "textDocument": { "uri": uri }
        }),
    }
}

/// Build a workspace/symbol request.
pub fn workspace_symbol_request(id: u64, query: &str) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0",
        id,
        method: "workspace/symbol".into(),
        params: serde_json::json!({
            "query": query
        }),
    }
}

/// Build a callHierarchy/incomingCalls request.
pub fn incoming_calls_request(id: u64, item: &serde_json::Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0",
        id,
        method: "callHierarchy/incomingCalls".into(),
        params: serde_json::json!({ "item": item }),
    }
}

/// Build a callHierarchy/outgoingCalls request.
pub fn outgoing_calls_request(id: u64, item: &serde_json::Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0",
        id,
        method: "callHierarchy/outgoingCalls".into(),
        params: serde_json::json!({ "item": item }),
    }
}

/// Build a textDocument/references request.
pub fn references_request(id: u64, uri: &str, line: u32, character: u32) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0",
        id,
        method: "textDocument/references".into(),
        params: serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": false }
        }),
    }
}

/// Build a shutdown request.
pub fn shutdown_request(id: u64) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0",
        id,
        method: "shutdown".into(),
        params: serde_json::json!(null),
    }
}

/// Build an exit notification.
pub fn exit_notification() -> RpcNotification {
    RpcNotification {
        jsonrpc: "2.0",
        method: "exit".into(),
        params: serde_json::json!(null),
    }
}
