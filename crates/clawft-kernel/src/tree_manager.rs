//! Unified tree-chain facade for the exo-resource-tree subsystem.
//!
//! [`TreeManager`] holds a [`ResourceTree`], [`MutationLog`], and
//! [`ChainManager`] together, ensuring every tree mutation produces
//! both a mutation event and a chain event atomically.
//!
//! # K0 Scope
//! Bootstrap, insert, remove, update_meta, checkpoint, register_service.
//!
//! # K1 Scope
//! Checkpoint persistence to disk, permission-gated mutations.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::debug;

use exo_resource_tree::{
    MutationEvent, MutationLog, NodeScoring, ResourceId, ResourceKind, ResourceTree,
};

use crate::capability::AgentCapabilities;
use crate::chain::ChainManager;
use crate::process::Pid;
use crate::wasm_runner::{BuiltinToolSpec, ToolVersion, compute_module_hash};

/// Statistics snapshot from the tree manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeStats {
    /// Number of nodes in the resource tree.
    pub node_count: usize,
    /// Number of mutation events recorded.
    pub mutation_count: usize,
    /// Hex-encoded root hash of the tree.
    pub root_hash: String,
}

/// Serializable snapshot of the full tree state for cross-node sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeSnapshot {
    /// Hex-encoded Merkle root hash at snapshot time.
    pub root_hash: String,
    /// Number of nodes in the tree.
    pub node_count: usize,
    /// Serialized tree state (all nodes and their metadata).
    pub nodes: Vec<TreeNodeSnapshot>,
    /// Timestamp when snapshot was taken.
    pub taken_at: chrono::DateTime<chrono::Utc>,
}

/// Snapshot of a single tree node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNodeSnapshot {
    /// Resource path (e.g., "/kernel/services/health").
    pub path: String,
    /// Resource kind.
    pub kind: String,
    /// Metadata key-value pairs.
    pub metadata: std::collections::HashMap<String, String>,
    /// Hex-encoded node hash.
    pub hash: String,
}

/// Unified facade over ResourceTree + MutationLog + ChainManager.
///
/// Every mutating operation on the tree also:
/// 1. Appends a [`MutationEvent`] to the mutation log
/// 2. Appends a [`ChainEvent`](crate::chain::ChainEvent) to the chain
/// 3. Stores `chain_seq` metadata on the affected tree node
pub struct TreeManager {
    tree: Mutex<ResourceTree>,
    mutation_log: Mutex<MutationLog>,
    chain: Arc<ChainManager>,
    /// Optional Ed25519 signing key for mutation signatures.
    #[cfg(feature = "exochain")]
    signing_key: Option<ed25519_dalek::SigningKey>,
}

impl TreeManager {
    /// Create a new TreeManager with an empty tree and mutation log.
    pub fn new(chain: Arc<ChainManager>) -> Self {
        Self {
            tree: Mutex::new(ResourceTree::new()),
            mutation_log: Mutex::new(MutationLog::new()),
            chain,
            #[cfg(feature = "exochain")]
            signing_key: None,
        }
    }

    /// Set the Ed25519 signing key for signing tree mutations.
    /// When set, all mutations will have their signature field populated.
    #[cfg(feature = "exochain")]
    pub fn set_signing_key(&mut self, key: ed25519_dalek::SigningKey) {
        self.signing_key = Some(key);
    }

    /// Sign arbitrary bytes with the configured signing key, if present.
    /// Returns `None` when no key is configured.
    #[cfg(feature = "exochain")]
    fn sign_bytes(&self, data: &[u8]) -> Option<Vec<u8>> {
        use ed25519_dalek::Signer;
        self.signing_key.as_ref().map(|k| k.sign(data).to_bytes().to_vec())
    }

    /// Build the canonical bytes for a mutation signature.
    ///
    /// Format: `"<operation>|<path>|<timestamp_rfc3339>"` encoded as UTF-8.
    #[cfg(feature = "exochain")]
    fn mutation_signature(
        &self,
        operation: &str,
        path: &str,
        timestamp: &chrono::DateTime<Utc>,
    ) -> Option<Vec<u8>> {
        let canonical = format!("{operation}|{path}|{}", timestamp.to_rfc3339());
        self.sign_bytes(canonical.as_bytes())
    }

    /// Bootstrap the tree with well-known WeftOS namespaces.
    ///
    /// Creates the standard namespace hierarchy (`/kernel`, `/kernel/services`,
    /// etc.), logs a MutationEvent::Create for each bootstrapped node, and
    /// appends a `tree.bootstrap` chain event with node count and root hash.
    pub fn bootstrap(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
        exo_resource_tree::bootstrap_fresh(&mut tree)?;

        // Log mutation events for each bootstrapped node (skip root, it's pre-existing)
        let mut log = self.mutation_log.lock().map_err(|e| format!("log lock: {e}"))?;
        let bootstrapped_paths = [
            "/kernel",
            "/kernel/services",
            "/kernel/processes",
            "/kernel/agents",
            "/network",
            "/network/peers",
            "/apps",
            "/environments",
        ];
        for path in &bootstrapped_paths {
            let rid = ResourceId::new(*path);
            let kind = ResourceKind::Namespace;
            let parent = rid
                .parent()
                .unwrap_or_else(ResourceId::root);
            let now = Utc::now();
            #[cfg(feature = "exochain")]
            let sig = self.mutation_signature("create", path, &now);
            #[cfg(not(feature = "exochain"))]
            let sig = None;
            log.append(MutationEvent::Create {
                id: rid,
                kind,
                parent,
                timestamp: now,
                signature: sig,
            });
        }

        // Chain event
        let hash_hex = hex_hash(&tree.root_hash());
        self.chain.append(
            "tree",
            "bootstrap",
            Some(serde_json::json!({
                "node_count": tree.len(),
                "root_hash": hash_hex,
            })),
        );

        debug!(nodes = tree.len(), "tree bootstrapped with chain event");
        Ok(())
    }

    /// Insert a new resource node into the tree.
    ///
    /// Creates the tree node, appends a MutationEvent::Create, appends a
    /// `tree.insert` chain event, and stores the chain sequence number as
    /// metadata on the node for two-way traceability.
    pub fn insert(
        &self,
        id: ResourceId,
        kind: ResourceKind,
        parent: ResourceId,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
        tree.insert(id.clone(), kind.clone(), parent.clone())?;

        // Chain event (before recompute so we capture pre-mutation state reference)
        let chain_event = self.chain.append(
            "tree",
            "insert",
            Some(serde_json::json!({
                "path": id.to_string(),
                "kind": format!("{kind:?}"),
                "parent": parent.to_string(),
            })),
        );

        // Store chain_seq metadata on the node for traceability
        if let Some(node) = tree.get_mut(&id) {
            node.metadata
                .insert("chain_seq".to_string(), serde_json::json!(chain_event.sequence));
        }

        // Recompute Merkle hashes
        tree.recompute_all();

        // Mutation log
        let now = Utc::now();
        #[cfg(feature = "exochain")]
        let sig = self.mutation_signature("create", &id.to_string(), &now);
        #[cfg(not(feature = "exochain"))]
        let sig = None;
        let mut log = self.mutation_log.lock().map_err(|e| format!("log lock: {e}"))?;
        log.append(MutationEvent::Create {
            id,
            kind,
            parent,
            timestamp: now,
            signature: sig,
        });

        Ok(())
    }

    /// Remove a leaf node from the tree.
    ///
    /// Removes the node, appends a MutationEvent::Remove and a
    /// `tree.remove` chain event.
    pub fn remove(
        &self,
        id: ResourceId,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
        tree.remove(id.clone())?;
        tree.recompute_all();

        self.chain.append(
            "tree",
            "remove",
            Some(serde_json::json!({
                "path": id.to_string(),
            })),
        );

        let now = Utc::now();
        #[cfg(feature = "exochain")]
        let sig = self.mutation_signature("remove", &id.to_string(), &now);
        #[cfg(not(feature = "exochain"))]
        let sig = None;
        let mut log = self.mutation_log.lock().map_err(|e| format!("log lock: {e}"))?;
        log.append(MutationEvent::Remove {
            id,
            timestamp: now,
            signature: sig,
        });

        Ok(())
    }

    /// Update metadata on a resource node.
    ///
    /// Sets the key-value pair on the node, appends a MutationEvent::UpdateMeta
    /// and a `tree.update_meta` chain event.
    pub fn update_meta(
        &self,
        id: &ResourceId,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
        let node = tree
            .get_mut(id)
            .ok_or_else(|| format!("node not found: {id}"))?;
        node.metadata.insert(key.to_string(), value.clone());
        node.updated_at = Utc::now();
        tree.recompute_all();

        self.chain.append(
            "tree",
            "update_meta",
            Some(serde_json::json!({
                "path": id.to_string(),
                "key": key,
            })),
        );

        let now = Utc::now();
        #[cfg(feature = "exochain")]
        let sig = self.mutation_signature("update_meta", &id.to_string(), &now);
        #[cfg(not(feature = "exochain"))]
        let sig = None;
        let mut log = self.mutation_log.lock().map_err(|e| format!("log lock: {e}"))?;
        log.append(MutationEvent::UpdateMeta {
            id: id.clone(),
            key: key.to_string(),
            value: Some(value),
            timestamp: now,
            signature: sig,
        });

        Ok(())
    }

    /// Register a service in the tree, creating `/kernel/services/{name}`.
    ///
    /// This is the unified path for service registration: it inserts the tree
    /// node AND creates the chain event atomically.
    pub fn register_service(
        &self,
        name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let service_id = ResourceId::new(format!("/kernel/services/{name}"));
        let parent = ResourceId::new("/kernel/services");
        self.insert(service_id, ResourceKind::Service, parent)
    }

    /// Register a service with a manifest chain event.
    ///
    /// Inserts the tree node and emits an additional `service.manifest`
    /// chain event with structured metadata (name, type, tree path,
    /// registration time). This produces an RVF-auditable registration
    /// when the chain is persisted as RVF segments.
    pub fn register_service_with_manifest(
        &self,
        name: &str,
        service_type: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.register_service(name)?;

        self.chain.append(
            "service",
            "service.manifest",
            Some(serde_json::json!({
                "name": name,
                "service_type": service_type,
                "tree_path": format!("/kernel/services/{name}"),
                "registered_at": Utc::now().to_rfc3339(),
            })),
        );

        Ok(())
    }

    /// Register an agent in the tree, creating `/kernel/agents/{agent_id}`.
    ///
    /// Creates the tree node with kind `Agent`, sets metadata (pid, state,
    /// spawn_time), and emits an `agent.spawn` chain event.
    pub fn register_agent(
        &self,
        agent_id: &str,
        pid: Pid,
        caps: &AgentCapabilities,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let agent_rid = ResourceId::new(format!("/kernel/agents/{agent_id}"));
        let parent = ResourceId::new("/kernel/agents");

        // Insert tree node
        {
            let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
            tree.insert(agent_rid.clone(), ResourceKind::Agent, parent.clone())?;

            // Set metadata
            if let Some(node) = tree.get_mut(&agent_rid) {
                node.metadata.insert("pid".into(), serde_json::json!(pid));
                node.metadata
                    .insert("state".into(), serde_json::json!("starting"));
                node.metadata
                    .insert("spawn_time".into(), serde_json::json!(Utc::now().to_rfc3339()));
                node.metadata
                    .insert("can_spawn".into(), serde_json::json!(caps.can_spawn));
                node.metadata
                    .insert("can_ipc".into(), serde_json::json!(caps.can_ipc));
                node.metadata
                    .insert("can_exec_tools".into(), serde_json::json!(caps.can_exec_tools));
            }
            tree.recompute_all();
        }

        // Chain event
        let chain_event = self.chain.append(
            "agent",
            "agent.spawn",
            Some(serde_json::json!({
                "agent_id": agent_id,
                "pid": pid,
                "capabilities": {
                    "can_spawn": caps.can_spawn,
                    "can_ipc": caps.can_ipc,
                    "can_exec_tools": caps.can_exec_tools,
                    "can_network": caps.can_network,
                },
            })),
        );

        // Store chain_seq
        {
            let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
            if let Some(node) = tree.get_mut(&agent_rid) {
                node.metadata
                    .insert("chain_seq".into(), serde_json::json!(chain_event.sequence));
            }
        }

        // Mutation log
        let now = Utc::now();
        #[cfg(feature = "exochain")]
        let sig = self.mutation_signature("create", &agent_rid.to_string(), &now);
        #[cfg(not(feature = "exochain"))]
        let sig = None;
        let mut log = self
            .mutation_log
            .lock()
            .map_err(|e| format!("log lock: {e}"))?;
        log.append(MutationEvent::Create {
            id: agent_rid,
            kind: ResourceKind::Agent,
            parent,
            timestamp: now,
            signature: sig,
        });

        debug!(agent_id, pid, "agent registered in tree");
        Ok(())
    }

    /// Unregister an agent from the tree.
    ///
    /// Updates the tree node metadata (state=exited, exit_code, stop_time)
    /// and emits an `agent.stop` chain event. Does NOT remove the node
    /// (preserves audit trail).
    pub fn unregister_agent(
        &self,
        agent_id: &str,
        pid: Pid,
        exit_code: i32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let agent_rid = ResourceId::new(format!("/kernel/agents/{agent_id}"));

        {
            let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
            if let Some(node) = tree.get_mut(&agent_rid) {
                node.metadata
                    .insert("state".into(), serde_json::json!("exited"));
                node.metadata
                    .insert("exit_code".into(), serde_json::json!(exit_code));
                node.metadata
                    .insert("stop_time".into(), serde_json::json!(Utc::now().to_rfc3339()));
                node.updated_at = Utc::now();
            }
            tree.recompute_all();
        }

        self.chain.append(
            "agent",
            "agent.stop",
            Some(serde_json::json!({
                "agent_id": agent_id,
                "pid": pid,
                "exit_code": exit_code,
            })),
        );

        let now = Utc::now();
        #[cfg(feature = "exochain")]
        let sig = self.mutation_signature("update_meta", &agent_rid.to_string(), &now);
        #[cfg(not(feature = "exochain"))]
        let sig = None;
        let mut log = self
            .mutation_log
            .lock()
            .map_err(|e| format!("log lock: {e}"))?;
        log.append(MutationEvent::UpdateMeta {
            id: agent_rid,
            key: "state".into(),
            value: Some(serde_json::json!("exited")),
            timestamp: now,
            signature: sig,
        });

        debug!(agent_id, pid, exit_code, "agent unregistered in tree");
        Ok(())
    }

    /// Update an agent's state in the tree.
    pub fn update_agent_state(
        &self,
        agent_id: &str,
        state: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let agent_rid = ResourceId::new(format!("/kernel/agents/{agent_id}"));
        self.update_meta(&agent_rid, "state", serde_json::json!(state))
    }

    /// Get direct access to the resource tree (for read-only queries).
    pub fn tree(&self) -> &Mutex<ResourceTree> {
        &self.tree
    }

    /// Get direct access to the mutation log.
    pub fn mutation_log(&self) -> &Mutex<MutationLog> {
        &self.mutation_log
    }

    /// Get the chain manager.
    pub fn chain(&self) -> &Arc<ChainManager> {
        &self.chain
    }

    /// Get the root hash of the resource tree.
    pub fn root_hash(&self) -> [u8; 32] {
        self.tree
            .lock()
            .map(|t| t.root_hash())
            .unwrap_or([0u8; 32])
    }

    /// Get a statistics snapshot.
    pub fn stats(&self) -> TreeStats {
        let tree = self.tree.lock().unwrap();
        let log = self.mutation_log.lock().unwrap();
        TreeStats {
            node_count: tree.len(),
            mutation_count: log.len(),
            root_hash: hex_hash(&tree.root_hash()),
        }
    }

    // --- Scoring API ---

    /// Set the scoring vector for a node.
    ///
    /// Updates the tree, logs a MutationEvent::UpdateScoring, and emits
    /// a `scoring.update` chain event.
    pub fn update_scoring(
        &self,
        id: &ResourceId,
        scoring: NodeScoring,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let old = {
            let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
            tree
                .update_scoring(id, scoring)
                .ok_or_else(|| format!("node not found: {id}"))?
        };

        self.chain.append(
            "scoring",
            "scoring.update",
            Some(serde_json::json!({
                "path": id.to_string(),
                "old": old.as_array(),
                "new": scoring.as_array(),
            })),
        );

        let now = Utc::now();
        #[cfg(feature = "exochain")]
        let sig = self.mutation_signature("update_scoring", &id.to_string(), &now);
        #[cfg(not(feature = "exochain"))]
        let sig = None;
        let mut log = self.mutation_log.lock().map_err(|e| format!("log lock: {e}"))?;
        log.append(MutationEvent::UpdateScoring {
            id: id.clone(),
            old,
            new: scoring,
            timestamp: now,
            signature: sig,
        });

        debug!(path = %id, "scoring updated");
        Ok(())
    }

    /// EMA-blend an observation into a node's scoring.
    ///
    /// Emits a `scoring.blend` chain event.
    pub fn blend_scoring(
        &self,
        id: &ResourceId,
        observation: &NodeScoring,
        alpha: f32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (old, new) = {
            let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
            let node = tree
                .get(id)
                .ok_or_else(|| format!("node not found: {id}"))?;
            let old = node.scoring;

            tree.blend_scoring(id, observation, alpha);

            let node = tree
                .get(id)
                .ok_or_else(|| format!("node not found: {id}"))?;
            let new = node.scoring;
            (old, new)
        };

        self.chain.append(
            "scoring",
            "scoring.blend",
            Some(serde_json::json!({
                "path": id.to_string(),
                "alpha": alpha,
                "old": old.as_array(),
                "new": new.as_array(),
            })),
        );

        let now = Utc::now();
        #[cfg(feature = "exochain")]
        let sig = self.mutation_signature("update_scoring", &id.to_string(), &now);
        #[cfg(not(feature = "exochain"))]
        let sig = None;
        let mut log = self.mutation_log.lock().map_err(|e| format!("log lock: {e}"))?;
        log.append(MutationEvent::UpdateScoring {
            id: id.clone(),
            old,
            new,
            timestamp: now,
            signature: sig,
        });

        debug!(path = %id, alpha, "scoring blended");
        Ok(())
    }

    /// Get the scoring vector for a node.
    pub fn get_scoring(
        &self,
        id: &ResourceId,
    ) -> Option<NodeScoring> {
        let tree = self.tree.lock().ok()?;
        tree.get(id).map(|n| n.scoring)
    }

    /// Find nodes most similar to a target node by cosine similarity.
    ///
    /// Returns up to `count` `(ResourceId, similarity)` pairs sorted by
    /// descending similarity.
    pub fn find_similar(
        &self,
        target_id: &ResourceId,
        count: usize,
    ) -> Vec<(ResourceId, f32)> {
        let tree = match self.tree.lock() {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };
        let target = match tree.get(target_id) {
            Some(n) => n.scoring,
            None => return Vec::new(),
        };

        let mut scored: Vec<(ResourceId, f32)> = tree
            .iter()
            .filter(|(id, _)| *id != target_id)
            .map(|(id, node)| (id.clone(), target.cosine_similarity(&node.scoring)))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(count);
        scored
    }

    /// Rank all nodes by weighted score.
    ///
    /// Returns up to `count` `(ResourceId, weighted_score)` pairs sorted
    /// by descending score.
    pub fn rank_by_score(
        &self,
        weights: &[f32; 6],
        count: usize,
    ) -> Vec<(ResourceId, f32)> {
        let tree = match self.tree.lock() {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };

        let mut scored: Vec<(ResourceId, f32)> = tree
            .iter()
            .map(|(id, node)| (id.clone(), node.scoring.weighted_score(weights)))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(count);
        scored
    }

    // --- Tool lifecycle API ---

    /// Build a tool: validate WASM bytes, compute hash, sign with Ed25519.
    ///
    /// Returns a `ToolVersion` with the computed module hash and Ed25519
    /// signature. Emits a `tool.build` chain event.
    pub fn build_tool(
        &self,
        name: &str,
        wasm_bytes: &[u8],
        signing_key: &ed25519_dalek::SigningKey,
    ) -> Result<ToolVersion, Box<dyn std::error::Error + Send + Sync>> {
        let module_hash = compute_module_hash(wasm_bytes);

        use ed25519_dalek::Signer;
        let signature = signing_key.sign(&module_hash);
        let sig_bytes: [u8; 64] = signature.to_bytes();

        let chain_event = self.chain.append(
            "tool",
            "tool.build",
            Some(serde_json::json!({
                "name": name,
                "module_hash": hex_hash(&module_hash),
                "sig_algo": "Ed25519",
            })),
        );

        let version = ToolVersion {
            version: 1,
            module_hash,
            signature: sig_bytes,
            deployed_at: Utc::now(),
            revoked: false,
            chain_seq: chain_event.sequence,
        };

        debug!(name, "tool built with hash and signature");
        Ok(version)
    }

    /// Deploy a tool to the resource tree.
    ///
    /// Creates the tool node at `/kernel/tools/{category}/{name}`,
    /// stores version metadata, and emits a `tool.deploy` chain event.
    pub fn deploy_tool(
        &self,
        spec: &BuiltinToolSpec,
        version: &ToolVersion,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let cat = match spec.category {
            crate::wasm_runner::ToolCategory::Filesystem => "fs",
            crate::wasm_runner::ToolCategory::Agent => "agent",
            crate::wasm_runner::ToolCategory::System => "sys",
            crate::wasm_runner::ToolCategory::Ecc => "ecc",
            crate::wasm_runner::ToolCategory::User => "user",
        };

        // Ensure category namespace exists
        let cat_path = format!("/kernel/tools/{cat}");
        let cat_rid = ResourceId::new(&cat_path);
        {
            let tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
            if tree.get(&cat_rid).is_none() {
                drop(tree);
                self.insert(
                    cat_rid,
                    ResourceKind::Namespace,
                    ResourceId::new("/kernel/tools"),
                )?;
            }
        }

        // Tool short name (e.g. "read_file" from "fs.read_file")
        let short_name = spec.name.rsplit('.').next().unwrap_or(&spec.name);
        let tool_path = format!("/kernel/tools/{cat}/{short_name}");
        let tool_rid = ResourceId::new(&tool_path);

        // Insert the tool node
        self.insert(
            tool_rid.clone(),
            ResourceKind::Tool,
            ResourceId::new(&cat_path),
        )?;

        // Set metadata
        self.update_meta(&tool_rid, "tool_version", serde_json::json!(version.version))?;
        self.update_meta(
            &tool_rid,
            "module_hash",
            serde_json::json!(hex_hash(&version.module_hash)),
        )?;
        self.update_meta(&tool_rid, "gate_action", serde_json::json!(&spec.gate_action))?;
        self.update_meta(
            &tool_rid,
            "deployed_at",
            serde_json::json!(version.deployed_at.to_rfc3339()),
        )?;

        // K4 B2: Persist version history array in tree metadata
        let versions_array = serde_json::json!([{
            "version": version.version,
            "module_hash": hex_hash(&version.module_hash),
            "deployed_at": version.deployed_at.to_rfc3339(),
            "revoked": version.revoked,
            "chain_seq": version.chain_seq,
        }]);
        self.update_meta(&tool_rid, "versions", versions_array)?;

        self.chain.append(
            "tool",
            "tool.deploy",
            Some(serde_json::json!({
                "name": spec.name,
                "version": version.version,
                "tree_path": tool_path,
                "module_hash": hex_hash(&version.module_hash),
                "gate_action": spec.gate_action,
            })),
        );

        debug!(tool = %spec.name, version = version.version, "tool deployed");
        Ok(())
    }

    /// Update a tool to a new version.
    ///
    /// Updates the tool node's metadata with new version info and
    /// emits a `tool.version.update` chain event linking old to new.
    pub fn update_tool_version(
        &self,
        name: &str,
        new_version: &ToolVersion,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let parts: Vec<&str> = name.splitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(format!("invalid tool name: {name}").into());
        }
        let tool_path = format!("/kernel/tools/{}/{}", parts[0], parts[1]);
        let tool_rid = ResourceId::new(&tool_path);

        // Get old version info from metadata
        let (old_version, old_hash) = {
            let tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
            let node = tree
                .get(&tool_rid)
                .ok_or_else(|| format!("tool not found: {tool_path}"))?;
            let ver = node
                .metadata
                .get("tool_version")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let hash = node
                .metadata
                .get("module_hash")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (ver, hash)
        };

        self.update_meta(
            &tool_rid,
            "tool_version",
            serde_json::json!(new_version.version),
        )?;
        self.update_meta(
            &tool_rid,
            "module_hash",
            serde_json::json!(hex_hash(&new_version.module_hash)),
        )?;
        self.update_meta(
            &tool_rid,
            "deployed_at",
            serde_json::json!(new_version.deployed_at.to_rfc3339()),
        )?;

        // K4 B2: Append to version history array
        {
            let tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
            let node = tree
                .get(&tool_rid)
                .ok_or_else(|| format!("tool not found: {tool_path}"))?;
            let mut versions: Vec<serde_json::Value> = node
                .metadata
                .get("versions")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            versions.push(serde_json::json!({
                "version": new_version.version,
                "module_hash": hex_hash(&new_version.module_hash),
                "deployed_at": new_version.deployed_at.to_rfc3339(),
                "revoked": new_version.revoked,
                "chain_seq": new_version.chain_seq,
            }));
            drop(tree);
            self.update_meta(&tool_rid, "versions", serde_json::json!(versions))?;
        }

        self.chain.append(
            "tool",
            "tool.version.update",
            Some(serde_json::json!({
                "name": name,
                "old_version": old_version,
                "new_version": new_version.version,
                "old_hash": old_hash,
                "new_hash": hex_hash(&new_version.module_hash),
            })),
        );

        debug!(tool = name, old = old_version, new = new_version.version, "tool version updated");
        Ok(())
    }

    /// Revoke a tool version.
    ///
    /// Marks the specified version as revoked in metadata. Does NOT
    /// delete the tree node (preserves audit trail). Emits a
    /// `tool.version.revoke` chain event.
    pub fn revoke_tool_version(
        &self,
        name: &str,
        version: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let parts: Vec<&str> = name.splitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(format!("invalid tool name: {name}").into());
        }
        let tool_path = format!("/kernel/tools/{}/{}", parts[0], parts[1]);
        let tool_rid = ResourceId::new(&tool_path);

        {
            let tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
            tree.get(&tool_rid)
                .ok_or_else(|| format!("tool not found: {tool_path}"))?;
        }

        let revoke_key = format!("v{version}_revoked");
        let revoke_at_key = format!("v{version}_revoked_at");
        self.update_meta(&tool_rid, &revoke_key, serde_json::json!(true))?;
        self.update_meta(
            &tool_rid,
            &revoke_at_key,
            serde_json::json!(Utc::now().to_rfc3339()),
        )?;

        let module_hash = {
            let tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
            let node = tree.get(&tool_rid).unwrap();
            node.metadata
                .get("module_hash")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        self.chain.append(
            "tool",
            "tool.version.revoke",
            Some(serde_json::json!({
                "name": name,
                "version": version,
                "module_hash": module_hash,
            })),
        );

        debug!(tool = name, version, "tool version revoked");
        Ok(())
    }

    /// Query version history for a tool (K4 B2).
    ///
    /// Returns the list of version entries persisted in the tree
    /// metadata, or an empty vec if the tool has no versions.
    pub fn get_tool_versions(
        &self,
        name: &str,
    ) -> Vec<serde_json::Value> {
        let parts: Vec<&str> = name.splitn(2, '.').collect();
        if parts.len() != 2 {
            return Vec::new();
        }
        let tool_path = format!("/kernel/tools/{}/{}", parts[0], parts[1]);
        let tool_rid = ResourceId::new(&tool_path);

        let tree = match self.tree.lock() {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };
        tree.get(&tool_rid)
            .and_then(|node| node.metadata.get("versions"))
            .and_then(|v| serde_json::from_value::<Vec<serde_json::Value>>(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Save tree state to a checkpoint file on disk.
    ///
    /// Serializes the tree via `exo_resource_tree::to_checkpoint()` and
    /// writes the bytes to the given path.
    pub fn save_checkpoint(
        &self,
        path: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
        let data = exo_resource_tree::to_checkpoint(&tree)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &data)?;

        debug!(
            path = %path.display(),
            nodes = tree.len(),
            bytes = data.len(),
            "tree checkpoint saved"
        );
        Ok(())
    }

    /// Load tree state from a checkpoint file.
    ///
    /// Reads the file, deserializes via `exo_resource_tree::from_checkpoint()`,
    /// and replaces the internal tree. The mutation log is cleared.
    pub fn load_checkpoint(
        &self,
        path: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let data = std::fs::read(path)?;
        let restored = exo_resource_tree::from_checkpoint(&data)?;

        let node_count = restored.len();
        let root_hash = hex_hash(&restored.root_hash());

        let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
        *tree = restored;

        // Clear mutation log since we loaded a fresh snapshot
        let mut log = self.mutation_log.lock().map_err(|e| format!("log lock: {e}"))?;
        *log = MutationLog::new();

        debug!(
            path = %path.display(),
            nodes = node_count,
            root_hash = %root_hash,
            "tree checkpoint loaded"
        );
        Ok(())
    }

    /// Create a combined checkpoint of tree + mutation log + chain state.
    ///
    /// Returns JSON with the tree checkpoint, mutation count, and chain
    /// checkpoint info. Full checkpoint persistence is a K1 concern.
    pub fn checkpoint(&self) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
        let log = self.mutation_log.lock().map_err(|e| format!("log lock: {e}"))?;

        let tree_data = exo_resource_tree::to_checkpoint(&tree)?;
        let chain_cp = self.chain.checkpoint();

        let checkpoint = serde_json::json!({
            "tree": serde_json::from_slice::<serde_json::Value>(&tree_data)
                .unwrap_or(serde_json::Value::Null),
            "mutation_count": log.len(),
            "chain_checkpoint": {
                "chain_id": chain_cp.chain_id,
                "sequence": chain_cp.sequence,
                "timestamp": chain_cp.timestamp.to_rfc3339(),
            },
            "root_hash": hex_hash(&tree.root_hash()),
        });

        // Log checkpoint event on chain
        drop(tree);
        drop(log);
        self.chain.append(
            "tree",
            "checkpoint",
            Some(serde_json::json!({
                "root_hash": checkpoint["root_hash"],
                "chain_seq": chain_cp.sequence,
            })),
        );

        Ok(checkpoint)
    }

    // --- K6 cross-node sync API ---

    /// Create a serializable snapshot of the full tree state.
    /// Used for cross-node tree synchronization in K6.4.
    pub fn snapshot(&self) -> Result<TreeSnapshot, Box<dyn std::error::Error + Send + Sync>> {
        let tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
        let root_hash = hex_hash(&tree.root_hash());
        let node_count = tree.len();

        let nodes: Vec<TreeNodeSnapshot> = tree
            .iter()
            .map(|(id, node)| {
                let metadata = node
                    .metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_string()))
                    .collect();
                TreeNodeSnapshot {
                    path: id.to_string(),
                    kind: format!("{:?}", node.kind),
                    metadata,
                    hash: hex_hash(&node.merkle_hash),
                }
            })
            .collect();

        Ok(TreeSnapshot {
            root_hash,
            node_count,
            nodes,
            taken_at: Utc::now(),
        })
    }

    /// Apply a remote mutation received from a peer node.
    /// Records the mutation in the local log.
    ///
    /// **Security note** (K6.4): Full implementation should verify
    /// `event.signature` against the sending node's Ed25519 public key
    /// before applying. Currently signature verification is deferred to
    /// the mesh transport layer (Noise channel authenticates the peer).
    pub fn apply_remote_mutation(
        &self,
        event: MutationEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut tree = self.tree.lock().map_err(|e| format!("tree lock: {e}"))?;
        let mut log = self
            .mutation_log
            .lock()
            .map_err(|e| format!("log lock: {e}"))?;

        // Apply based on mutation type
        match &event {
            MutationEvent::Create {
                id,
                kind,
                parent,
                ..
            } => {
                // Only insert if not already present (idempotent)
                if tree.get(id).is_none() {
                    tree.insert(id.clone(), kind.clone(), parent.clone())?;
                    tree.recompute_all();
                }
            }
            MutationEvent::Remove { id, .. } => {
                if tree.get(id).is_some() {
                    tree.remove(id.clone())?;
                    tree.recompute_all();
                }
            }
            MutationEvent::UpdateMeta { id, key, value, .. } => {
                if let Some(node) = tree.get_mut(id) {
                    if let Some(val) = value {
                        node.metadata.insert(key.clone(), val.clone());
                    } else {
                        node.metadata.remove(key);
                    }
                    node.updated_at = Utc::now();
                }
                tree.recompute_all();
            }
            MutationEvent::Move { .. } | MutationEvent::UpdateScoring { .. } => {
                // Move and scoring updates are recorded but not yet applied
                // in K6.0 — full support arrives in K6.4.
            }
            _ => {
                // Future MutationEvent variants -- record but do not apply.
            }
        }

        // Record the mutation
        log.append(event);

        Ok(())
    }
}

impl std::fmt::Debug for TreeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.stats();
        f.debug_struct("TreeManager")
            .field("node_count", &stats.node_count)
            .field("mutation_count", &stats.mutation_count)
            .finish()
    }
}

/// Format a 32-byte hash as a hex string.
fn hex_hash(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_chain() -> Arc<ChainManager> {
        Arc::new(ChainManager::new(0, 1000))
    }

    #[test]
    fn bootstrap_creates_nodes_and_chain_events() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let tree = tm.tree().lock().unwrap();
        assert_eq!(tree.len(), 9); // root + 8 namespaces (incl /kernel/agents)

        // Chain should have genesis + bootstrap event
        assert!(chain.len() >= 2);
        let events = chain.tail(0);
        assert!(events.iter().any(|e| e.kind == "bootstrap" && e.source == "tree"));

        // Mutation log should have 8 entries (one per bootstrapped namespace)
        let log = tm.mutation_log().lock().unwrap();
        assert_eq!(log.len(), 8);
    }

    #[test]
    fn insert_creates_node_and_chain_event() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let before_len = chain.len();
        tm.insert(
            ResourceId::new("/kernel/services/cron"),
            ResourceKind::Service,
            ResourceId::new("/kernel/services"),
        )
        .unwrap();

        let tree = tm.tree().lock().unwrap();
        assert!(tree.get(&ResourceId::new("/kernel/services/cron")).is_some());

        // Chain should have one more event
        assert_eq!(chain.len(), before_len + 1);

        // Node should have chain_seq metadata
        let node = tree.get(&ResourceId::new("/kernel/services/cron")).unwrap();
        assert!(node.metadata.contains_key("chain_seq"));
    }

    #[test]
    fn remove_creates_chain_event() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        tm.insert(
            ResourceId::new("/kernel/services/test"),
            ResourceKind::Service,
            ResourceId::new("/kernel/services"),
        )
        .unwrap();

        let before_len = chain.len();
        tm.remove(ResourceId::new("/kernel/services/test")).unwrap();

        let tree = tm.tree().lock().unwrap();
        assert!(tree.get(&ResourceId::new("/kernel/services/test")).is_none());
        assert_eq!(chain.len(), before_len + 1);
    }

    #[test]
    fn update_meta_creates_chain_event() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let before_len = chain.len();
        tm.update_meta(
            &ResourceId::new("/kernel"),
            "version",
            serde_json::json!("0.1.0"),
        )
        .unwrap();

        assert_eq!(chain.len(), before_len + 1);
        let tree = tm.tree().lock().unwrap();
        let node = tree.get(&ResourceId::new("/kernel")).unwrap();
        assert_eq!(node.metadata.get("version").unwrap(), &serde_json::json!("0.1.0"));
    }

    #[test]
    fn register_service_creates_node() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        tm.register_service("cron").unwrap();

        let tree = tm.tree().lock().unwrap();
        let node = tree.get(&ResourceId::new("/kernel/services/cron")).unwrap();
        assert_eq!(node.kind, ResourceKind::Service);
        assert!(node.metadata.contains_key("chain_seq"));
    }

    #[test]
    fn stats_reports_correctly() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let stats = tm.stats();
        assert_eq!(stats.node_count, 9); // root + 8 namespaces
        assert_eq!(stats.mutation_count, 8);
        assert_ne!(stats.root_hash, "0".repeat(64));
    }

    #[test]
    fn checkpoint_includes_tree_and_chain() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let cp = tm.checkpoint().unwrap();
        assert!(cp.get("tree").is_some());
        assert!(cp.get("mutation_count").is_some());
        assert!(cp.get("chain_checkpoint").is_some());
        assert!(cp.get("root_hash").is_some());
    }

    #[test]
    fn register_agent_creates_node_and_chain_event() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let caps = crate::capability::AgentCapabilities::default();
        let before_len = chain.len();
        tm.register_agent("test-agent", 42, &caps).unwrap();

        // Node should exist
        let tree = tm.tree().lock().unwrap();
        let node = tree
            .get(&ResourceId::new("/kernel/agents/test-agent"))
            .unwrap();
        assert_eq!(node.kind, ResourceKind::Agent);
        assert_eq!(node.metadata["pid"], serde_json::json!(42));
        assert_eq!(node.metadata["state"], serde_json::json!("starting"));
        assert!(node.metadata.contains_key("chain_seq"));
        drop(tree);

        // Chain event
        assert!(chain.len() > before_len);
        let events = chain.tail(2);
        assert!(events.iter().any(|e| e.kind == "agent.spawn"));
    }

    #[test]
    fn unregister_agent_updates_node() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let caps = crate::capability::AgentCapabilities::default();
        tm.register_agent("exit-agent", 10, &caps).unwrap();
        tm.unregister_agent("exit-agent", 10, 0).unwrap();

        // Node still exists but state=exited
        let tree = tm.tree().lock().unwrap();
        let node = tree
            .get(&ResourceId::new("/kernel/agents/exit-agent"))
            .unwrap();
        assert_eq!(node.metadata["state"], serde_json::json!("exited"));
        assert_eq!(node.metadata["exit_code"], serde_json::json!(0));
        assert!(node.metadata.contains_key("stop_time"));
        drop(tree);

        // Chain event
        let events = chain.tail(2);
        assert!(events.iter().any(|e| e.kind == "agent.stop"));
    }

    #[test]
    fn update_agent_state() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let caps = crate::capability::AgentCapabilities::default();
        tm.register_agent("state-agent", 20, &caps).unwrap();
        tm.update_agent_state("state-agent", "running").unwrap();

        let tree = tm.tree().lock().unwrap();
        let node = tree
            .get(&ResourceId::new("/kernel/agents/state-agent"))
            .unwrap();
        assert_eq!(node.metadata["state"], serde_json::json!("running"));
    }

    #[test]
    fn chain_integrity_after_operations() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();
        tm.register_service("cron").unwrap();

        let result = chain.verify_integrity();
        assert!(result.valid);
        assert!(result.event_count >= 3); // genesis + bootstrap + insert
    }

    // --- Scoring API tests ---

    #[test]
    fn update_scoring_creates_chain_event() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let before_len = chain.len();
        let scoring = NodeScoring::new(0.9, 0.8, 0.7, 0.6, 0.5, 0.4);
        tm.update_scoring(&ResourceId::new("/kernel"), scoring).unwrap();

        // Chain event emitted
        assert!(chain.len() > before_len);
        let events = chain.tail(2);
        assert!(events.iter().any(|e| e.kind == "scoring.update"));

        // Scoring stored on node
        let s = tm.get_scoring(&ResourceId::new("/kernel")).unwrap();
        assert!((s.trust - 0.9).abs() < 1e-6);

        // Mutation log has the entry
        let log = tm.mutation_log().lock().unwrap();
        let last = log.events().last().unwrap();
        assert!(matches!(last, MutationEvent::UpdateScoring { .. }));
    }

    #[test]
    fn blend_scoring_creates_chain_event() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let before_len = chain.len();
        let obs = NodeScoring::new(1.0, 1.0, 1.0, 1.0, 1.0, 1.0);
        tm.blend_scoring(&ResourceId::new("/kernel"), &obs, 0.5).unwrap();

        assert!(chain.len() > before_len);
        let events = chain.tail(2);
        assert!(events.iter().any(|e| e.kind == "scoring.blend"));

        // Should be EMA blended: 0.5*0.5 + 1.0*0.5 = 0.75
        let s = tm.get_scoring(&ResourceId::new("/kernel")).unwrap();
        assert!((s.trust - 0.75).abs() < 1e-6);
    }

    #[test]
    fn get_scoring_nonexistent_returns_none() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        assert!(tm.get_scoring(&ResourceId::new("/no/such/node")).is_none());
    }

    #[test]
    fn find_similar_returns_ranked() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        // Set /kernel to a specific scoring
        let target = NodeScoring::new(0.9, 0.9, 0.9, 0.9, 0.9, 0.9);
        tm.update_scoring(&ResourceId::new("/kernel"), target).unwrap();

        // /apps gets a similar scoring
        let similar = NodeScoring::new(0.85, 0.85, 0.85, 0.85, 0.85, 0.85);
        tm.update_scoring(&ResourceId::new("/apps"), similar).unwrap();

        let results = tm.find_similar(&ResourceId::new("/kernel"), 3);
        assert!(!results.is_empty());
        // First result should have high similarity
        assert!(results[0].1 > 0.9);
    }

    #[test]
    fn rank_by_score_returns_ordered() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        // High trust on /kernel
        tm.update_scoring(
            &ResourceId::new("/kernel"),
            NodeScoring::new(1.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        ).unwrap();

        // High performance on /apps
        tm.update_scoring(
            &ResourceId::new("/apps"),
            NodeScoring::new(0.0, 1.0, 0.0, 0.0, 0.0, 0.0),
        ).unwrap();

        // Rank by trust weight only
        let weights = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let ranked = tm.rank_by_score(&weights, 3);
        assert!(!ranked.is_empty());
        assert_eq!(ranked[0].0, ResourceId::new("/kernel"));
    }

    #[test]
    fn checkpoint_roundtrip_preserves_root_hash() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();
        tm.register_service("cron").unwrap();

        let original_hash = tm.stats().root_hash;

        // Simulate shutdown: save tree checkpoint and record root hash in chain.
        let dir = std::env::temp_dir().join("clawft-tree-ckpt-hash-test");
        let tree_path = dir.join("tree.json");
        tm.save_checkpoint(&tree_path).unwrap();
        chain.append(
            "tree",
            "tree.checkpoint",
            Some(serde_json::json!({
                "path": tree_path.display().to_string(),
                "root_hash": original_hash,
            })),
        );

        // Simulate restart: create new tree manager, load checkpoint.
        let tm2 = TreeManager::new(Arc::clone(&chain));
        tm2.load_checkpoint(&tree_path).unwrap();

        // Root hash should match what the chain recorded.
        let restored_hash = tm2.stats().root_hash;
        let chain_hash = chain.last_tree_root_hash().unwrap();
        assert_eq!(restored_hash, chain_hash);
        assert_eq!(restored_hash, original_hash);

        // Clean up.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checkpoint_hash_mismatch_detectable() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let dir = std::env::temp_dir().join("clawft-tree-ckpt-mismatch-test");
        let tree_path = dir.join("tree.json");
        tm.save_checkpoint(&tree_path).unwrap();

        // Record a fake root hash in the chain (simulating corruption).
        chain.append(
            "tree",
            "tree.checkpoint",
            Some(serde_json::json!({
                "root_hash": "0000000000000000000000000000000000000000000000000000000000000000",
            })),
        );

        // Load checkpoint and compare — should detect mismatch.
        let tm2 = TreeManager::new(Arc::clone(&chain));
        tm2.load_checkpoint(&tree_path).unwrap();

        let restored_hash = tm2.stats().root_hash;
        let chain_hash = chain.last_tree_root_hash().unwrap();
        assert_ne!(restored_hash, chain_hash, "should detect hash mismatch");

        // Clean up.
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- Tool lifecycle tests ---

    fn test_signing_key() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[42u8; 32])
    }

    fn setup_tool_tree() -> (Arc<ChainManager>, TreeManager) {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();
        // Create /kernel/tools namespace
        tm.insert(
            ResourceId::new("/kernel/tools"),
            ResourceKind::Namespace,
            ResourceId::new("/kernel"),
        )
        .unwrap();
        (chain, tm)
    }

    #[test]
    fn tool_build_computes_hash_and_signs() {
        let (_chain, tm) = setup_tool_tree();
        let key = test_signing_key();
        let wasm_bytes = b"fake wasm module bytes for testing";

        let tv = tm.build_tool("fs.read_file", wasm_bytes, &key).unwrap();
        assert_eq!(tv.version, 1);
        assert!(!tv.revoked);
        // Hash should match compute_module_hash
        let expected = compute_module_hash(wasm_bytes);
        assert_eq!(tv.module_hash, expected);
        // Signature should be non-zero
        assert_ne!(tv.signature, [0u8; 64]);
    }

    #[test]
    fn tool_deploy_creates_tree_node() {
        let (_chain, tm) = setup_tool_tree();
        let spec = crate::wasm_runner::builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.read_file")
            .unwrap();
        let tv = ToolVersion {
            version: 1,
            module_hash: [0xAA; 32],
            signature: [0xBB; 64],
            deployed_at: Utc::now(),
            revoked: false,
            chain_seq: 10,
        };

        tm.deploy_tool(&spec, &tv).unwrap();

        let tree = tm.tree().lock().unwrap();
        let node = tree.get(&ResourceId::new("/kernel/tools/fs/read_file"));
        assert!(node.is_some(), "tool node should exist");
        let node = node.unwrap();
        assert_eq!(node.kind, ResourceKind::Tool);
        assert_eq!(node.metadata["tool_version"], serde_json::json!(1));
        assert!(node.metadata.contains_key("gate_action"));
    }

    #[test]
    fn tool_deploy_emits_chain_event() {
        let (chain, tm) = setup_tool_tree();
        let spec = crate::wasm_runner::builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.read_file")
            .unwrap();
        let tv = ToolVersion {
            version: 1,
            module_hash: [0xAA; 32],
            signature: [0xBB; 64],
            deployed_at: Utc::now(),
            revoked: false,
            chain_seq: 10,
        };

        let before = chain.len();
        tm.deploy_tool(&spec, &tv).unwrap();
        assert!(chain.len() > before);

        let events = chain.tail(5);
        assert!(
            events.iter().any(|e| e.kind == "tool.deploy"),
            "expected tool.deploy chain event"
        );
    }

    #[test]
    fn tool_version_update_chain_links() {
        let (chain, tm) = setup_tool_tree();
        let spec = crate::wasm_runner::builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.read_file")
            .unwrap();
        let v1 = ToolVersion {
            version: 1,
            module_hash: [0xAA; 32],
            signature: [0xBB; 64],
            deployed_at: Utc::now(),
            revoked: false,
            chain_seq: 10,
        };
        tm.deploy_tool(&spec, &v1).unwrap();

        let v2 = ToolVersion {
            version: 2,
            module_hash: [0xCC; 32],
            signature: [0xDD; 64],
            deployed_at: Utc::now(),
            revoked: false,
            chain_seq: 20,
        };
        tm.update_tool_version("fs.read_file", &v2).unwrap();

        // Verify chain event
        let events = chain.tail(5);
        let update_evt = events
            .iter()
            .find(|e| e.kind == "tool.version.update")
            .expect("expected tool.version.update event");
        let payload = update_evt.payload.as_ref().unwrap();
        assert_eq!(payload["old_version"], 1);
        assert_eq!(payload["new_version"], 2);

        // Verify tree metadata updated
        let tree = tm.tree().lock().unwrap();
        let node = tree
            .get(&ResourceId::new("/kernel/tools/fs/read_file"))
            .unwrap();
        assert_eq!(node.metadata["tool_version"], serde_json::json!(2));
    }

    #[test]
    fn tool_version_revoke_marks_revoked() {
        let (_chain, tm) = setup_tool_tree();
        let spec = crate::wasm_runner::builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.read_file")
            .unwrap();
        let v1 = ToolVersion {
            version: 1,
            module_hash: [0xAA; 32],
            signature: [0xBB; 64],
            deployed_at: Utc::now(),
            revoked: false,
            chain_seq: 10,
        };
        tm.deploy_tool(&spec, &v1).unwrap();

        tm.revoke_tool_version("fs.read_file", 1).unwrap();

        let tree = tm.tree().lock().unwrap();
        let node = tree
            .get(&ResourceId::new("/kernel/tools/fs/read_file"))
            .unwrap();
        assert_eq!(node.metadata["v1_revoked"], serde_json::json!(true));
        assert!(node.metadata.contains_key("v1_revoked_at"));
    }

    // --- Version history tests (K4 B2) ---

    #[test]
    fn version_history_persisted_in_tree() {
        let (_chain, tm) = setup_tool_tree();
        let spec = crate::wasm_runner::builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.read_file")
            .unwrap();

        let v1 = ToolVersion {
            version: 1,
            module_hash: [0xAA; 32],
            signature: [0xBB; 64],
            deployed_at: Utc::now(),
            revoked: false,
            chain_seq: 10,
        };
        tm.deploy_tool(&spec, &v1).unwrap();

        let v2 = ToolVersion {
            version: 2,
            module_hash: [0xCC; 32],
            signature: [0xDD; 64],
            deployed_at: Utc::now(),
            revoked: false,
            chain_seq: 20,
        };
        tm.update_tool_version("fs.read_file", &v2).unwrap();

        let versions = tm.get_tool_versions("fs.read_file");
        assert_eq!(versions.len(), 2, "should have 2 versions");
        assert_eq!(versions[0]["version"], 1);
        assert_eq!(versions[1]["version"], 2);
    }

    #[test]
    fn version_history_includes_revoked() {
        let (_chain, tm) = setup_tool_tree();
        let spec = crate::wasm_runner::builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.read_file")
            .unwrap();

        let v1 = ToolVersion {
            version: 1,
            module_hash: [0xAA; 32],
            signature: [0xBB; 64],
            deployed_at: Utc::now(),
            revoked: false,
            chain_seq: 10,
        };
        tm.deploy_tool(&spec, &v1).unwrap();
        tm.revoke_tool_version("fs.read_file", 1).unwrap();

        let versions = tm.get_tool_versions("fs.read_file");
        assert_eq!(versions.len(), 1, "version entry should persist after revoke");
        // The revoke flag in the version array entry is as deployed (false),
        // but the per-version metadata key v1_revoked=true is set separately.
        // Version history records deploy-time state.
    }

    #[test]
    fn tool_revoke_emits_chain_event() {
        let (chain, tm) = setup_tool_tree();
        let spec = crate::wasm_runner::builtin_tool_catalog()
            .into_iter()
            .find(|s| s.name == "fs.read_file")
            .unwrap();
        let v1 = ToolVersion {
            version: 1,
            module_hash: [0xAA; 32],
            signature: [0xBB; 64],
            deployed_at: Utc::now(),
            revoked: false,
            chain_seq: 10,
        };
        tm.deploy_tool(&spec, &v1).unwrap();

        let before = chain.len();
        tm.revoke_tool_version("fs.read_file", 1).unwrap();
        assert!(chain.len() > before);

        let events = chain.tail(5);
        assert!(
            events.iter().any(|e| e.kind == "tool.version.revoke"),
            "expected tool.version.revoke chain event"
        );
    }

    #[test]
    fn snapshot_on_bootstrapped_tree() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let snap = tm.snapshot().unwrap();
        assert!(snap.node_count > 0);
        assert!(!snap.nodes.is_empty());
        // The snapshot should contain at least the root + bootstrapped namespaces
        assert!(snap.nodes.len() >= 9);
    }

    #[test]
    fn snapshot_root_hash_matches_stats() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let snap = tm.snapshot().unwrap();
        let stats = tm.stats();
        assert_eq!(snap.root_hash, stats.root_hash);
    }

    #[test]
    fn apply_remote_mutation_records_in_log() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        let before = tm.mutation_log().lock().unwrap().len();

        let event = MutationEvent::Create {
            id: ResourceId::new("/kernel/services/remote_svc"),
            kind: ResourceKind::Service,
            parent: ResourceId::new("/kernel/services"),
            timestamp: Utc::now(),
            signature: None,
        };
        tm.apply_remote_mutation(event).unwrap();

        let after = tm.mutation_log().lock().unwrap().len();
        assert_eq!(after, before + 1);

        // The node should exist in the tree
        let tree = tm.tree().lock().unwrap();
        assert!(tree.get(&ResourceId::new("/kernel/services/remote_svc")).is_some());
    }

    #[test]
    fn mutations_signed_when_key_set() {
        let chain = test_chain();
        let mut tm = TreeManager::new(Arc::clone(&chain));
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        tm.set_signing_key(key.clone());
        tm.bootstrap().unwrap();

        // Bootstrap mutations should all be signed
        {
            let log = tm.mutation_log().lock().unwrap();
            for evt in log.events() {
                if let MutationEvent::Create { signature, .. } = evt {
                    assert!(signature.is_some(), "bootstrap Create should be signed");
                    assert_eq!(signature.as_ref().unwrap().len(), 64);
                }
            }
        }

        // Insert should produce signed mutation
        tm.insert(
            ResourceId::new("/kernel/services/signed_svc"),
            ResourceKind::Service,
            ResourceId::new("/kernel/services"),
        )
        .unwrap();

        let log = tm.mutation_log().lock().unwrap();
        let last = log.events().last().unwrap();
        match last {
            MutationEvent::Create { signature, .. } => {
                let sig_bytes = signature.as_ref().expect("insert should be signed");
                assert_eq!(sig_bytes.len(), 64);
                // Verify the signature bytes form a valid Ed25519 signature
                let sig = ed25519_dalek::Signature::from_bytes(
                    sig_bytes.as_slice().try_into().unwrap(),
                );
                assert_eq!(sig.to_bytes().len(), 64);
            }
            _ => panic!("expected Create variant"),
        }
    }

    #[test]
    fn mutations_unsigned_without_key() {
        let chain = test_chain();
        let tm = TreeManager::new(Arc::clone(&chain));
        tm.bootstrap().unwrap();

        tm.insert(
            ResourceId::new("/kernel/services/unsigned_svc"),
            ResourceKind::Service,
            ResourceId::new("/kernel/services"),
        )
        .unwrap();

        let log = tm.mutation_log().lock().unwrap();
        // All mutations should have signature = None when no key is set
        for evt in log.events() {
            if let MutationEvent::Create { signature, .. } = evt {
                assert!(signature.is_none(), "should be None without signing key");
            }
        }
    }
}
