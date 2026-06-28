//! LSP server lifecycle — spawn, initialize, query, shutdown.

use std::io::{BufReader, BufWriter};
use std::process::{Child, Command, Stdio};

use crate::config::LanguageConfig;
use crate::protocol::*;

/// A running LSP server process.
pub struct LspServer {
    process: Child,
    writer: BufWriter<std::process::ChildStdin>,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
    pub initialized: bool,
}

impl LspServer {
    /// Spawn and initialize an LSP server.
    pub fn start(config: &LanguageConfig, root_path: &str) -> anyhow::Result<Self> {
        let root_uri = format!("file://{}", std::fs::canonicalize(root_path)?.display());

        tracing::info!(
            command = config.command,
            language = config.name,
            "spawning LSP server"
        );

        let mut process = Command::new(&config.command)
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to spawn LSP server '{}': {}. Is it installed?",
                    config.command,
                    e
                )
            })?;

        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("no stdin"))?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("no stdout"))?;

        let mut server = Self {
            process,
            writer: BufWriter::new(stdin),
            reader: BufReader::new(stdout),
            next_id: 1,
            initialized: false,
        };

        // Initialize handshake.
        let init_req =
            initialize_request(server.next_id(), &root_uri, &config.initialization_options);
        server.send_request(&init_req)?;
        let _resp = server.read_response()?;

        // Send initialized notification.
        let notif = initialized_notification();
        let body = serde_json::to_vec(&notif)?;
        write_message(&mut server.writer, &body)?;

        server.initialized = true;
        tracing::info!(language = config.name, "LSP server initialized");

        Ok(server)
    }

    /// Send a request and return the response.
    pub fn request(&mut self, req: &RpcRequest) -> anyhow::Result<serde_json::Value> {
        self.send_request(req)?;
        let resp = self.read_response()?;
        match resp.result {
            Some(v) => Ok(v),
            None => {
                let err = resp
                    .error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "no result".into());
                Err(anyhow::anyhow!("LSP error: {}", err))
            }
        }
    }

    /// Get document symbols for a file.
    pub fn document_symbols(&mut self, file_uri: &str) -> anyhow::Result<serde_json::Value> {
        let req = document_symbol_request(self.next_id(), file_uri);
        self.request(&req)
    }

    /// Get workspace symbols matching a query.
    pub fn workspace_symbols(&mut self, query: &str) -> anyhow::Result<serde_json::Value> {
        let req = workspace_symbol_request(self.next_id(), query);
        self.request(&req)
    }

    /// Get references to a symbol at a position.
    pub fn references(
        &mut self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> anyhow::Result<serde_json::Value> {
        let req = references_request(self.next_id(), uri, line, character);
        self.request(&req)
    }

    /// Gracefully shut down the server.
    pub fn shutdown(mut self) -> anyhow::Result<()> {
        if self.initialized {
            let req = shutdown_request(self.next_id());
            let _ = self.send_request(&req);
            let _ = self.read_response();

            let notif = exit_notification();
            let body = serde_json::to_vec(&notif).unwrap_or_default();
            let _ = write_message(&mut self.writer, &body);
        }
        let _ = self.process.wait();
        Ok(())
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send_request(&mut self, req: &RpcRequest) -> anyhow::Result<()> {
        let body = serde_json::to_vec(req)?;
        write_message(&mut self.writer, &body)?;
        Ok(())
    }

    fn read_response(&mut self) -> anyhow::Result<RpcResponse> {
        // LSP servers may send notifications between responses. Skip them.
        loop {
            let msg = read_message(&mut self.reader)?;
            let parsed: serde_json::Value = serde_json::from_str(&msg)?;

            // Notifications have no "id" field.
            if parsed.get("id").is_some() {
                let resp: RpcResponse = serde_json::from_value(parsed)?;
                return Ok(resp);
            }
            // Skip notifications (e.g., progress, diagnostics).
        }
    }
}

impl Drop for LspServer {
    fn drop(&mut self) {
        let _ = self.process.kill();
    }
}
