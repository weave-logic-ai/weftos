//! WEFT-71: `clawft.plugin.json` schema roundtrip + version-compat tests.
//!
//! Integration-level contract tests for the public [`PluginManifest`] API.
//! Unit tests live next to the type in `src/manifest.rs`; this file locks
//! the **public wire format** as a separate crate consumer would see it.
//!
//! Acceptance (wave-0d / WEFT-71):
//! - Roundtrip: known-good manifest → deserialize → serialize → stable
//! - Forward-compat: future / unknown fields ignored gracefully
//! - Negative: malformed JSON, missing required fields, wrong version

use clawft_plugin::{
    PluginCapability, PluginError, PluginManifest, PluginPermissions, PluginResourceConfig,
    VoiceCapability,
};
use serde_json::{Value, json};

/// Fully-populated known-good `clawft.plugin.json` body.
fn known_good_manifest_json() -> Value {
    json!({
        "id": "com.example.test-plugin",
        "name": "Test Plugin",
        "version": "1.2.3",
        "capabilities": ["tool", "skill"],
        "permissions": {
            "network": ["api.example.com"],
            "filesystem": ["/tmp/plugin"],
            "env_vars": ["MY_API_KEY"],
            "shell": false
        },
        "resources": {
            "max_fuel": 500_000_000u64,
            "max_memory_mb": 8,
            "max_http_requests_per_minute": 5,
            "max_log_messages_per_minute": 50,
            "max_execution_seconds": 15,
            "max_table_elements": 5000
        },
        "wasm_module": "plugin.wasm",
        "skills": ["code-review"],
        "tools": ["lint_code"],
        "voice": {
            "read_transcripts": true,
            "dispatch_commands": false,
            "synthesize_audio": true,
            "transcript_topics": ["weftos.voice.transcripts.v1"]
        }
    })
}

fn assert_manifests_equal(a: &PluginManifest, b: &PluginManifest) {
    assert_eq!(a.id, b.id);
    assert_eq!(a.name, b.name);
    assert_eq!(a.version, b.version);
    assert_eq!(a.capabilities, b.capabilities);
    assert_eq!(a.permissions, b.permissions);
    assert_eq!(a.resources.max_fuel, b.resources.max_fuel);
    assert_eq!(a.resources.max_memory_mb, b.resources.max_memory_mb);
    assert_eq!(
        a.resources.max_http_requests_per_minute,
        b.resources.max_http_requests_per_minute
    );
    assert_eq!(
        a.resources.max_log_messages_per_minute,
        b.resources.max_log_messages_per_minute
    );
    assert_eq!(
        a.resources.max_execution_seconds,
        b.resources.max_execution_seconds
    );
    assert_eq!(a.resources.max_table_elements, b.resources.max_table_elements);
    assert_eq!(a.wasm_module, b.wasm_module);
    assert_eq!(a.skills, b.skills);
    assert_eq!(a.tools, b.tools);
    assert_eq!(a.voice, b.voice);
}

// ── Roundtrip ───────────────────────────────────────────────────────────

/// Known-good JSON → deserialize → serialize → deserialize is lossless.
#[test]
fn roundtrip_known_good_manifest_is_lossless() {
    let raw = known_good_manifest_json().to_string();
    let first = PluginManifest::from_json(&raw).expect("parse known-good");
    let serialized = serde_json::to_string(&first).expect("serialize");
    let second = PluginManifest::from_json(&serialized).expect("re-parse");
    assert_manifests_equal(&first, &second);

    // Spot-check required + optional fields from the fixture.
    assert_eq!(first.id, "com.example.test-plugin");
    assert_eq!(first.name, "Test Plugin");
    assert_eq!(first.version, "1.2.3");
    assert_eq!(
        first.capabilities,
        vec![PluginCapability::Tool, PluginCapability::Skill]
    );
    assert_eq!(first.wasm_module.as_deref(), Some("plugin.wasm"));
    assert_eq!(first.skills, vec!["code-review"]);
    assert_eq!(first.tools, vec!["lint_code"]);
    let voice = first.voice.as_ref().expect("voice section");
    assert!(voice.read_transcripts);
    assert!(!voice.dispatch_commands);
    assert!(voice.synthesize_audio);
    assert_eq!(voice.transcript_topics, vec!["weftos.voice.transcripts.v1"]);
}

/// Second-hop serialize is **byte-equal**: once the value has been through
/// the typed struct, re-serializing the same value must not drift.
#[test]
fn roundtrip_second_hop_serialize_is_byte_equal() {
    let raw = known_good_manifest_json().to_string();
    let first = PluginManifest::from_json(&raw).expect("parse");
    let hop1 = serde_json::to_string(&first).expect("serialize hop1");
    let second = PluginManifest::from_json(&hop1).expect("re-parse");
    let hop2 = serde_json::to_string(&second).expect("serialize hop2");
    assert_eq!(
        hop1, hop2,
        "second-hop JSON must be byte-equal (stable public contract)"
    );
}

/// Value-level roundtrip: deserialize → serialize → `Value` equals a
/// re-serialize of the same struct (canonical field set).
#[test]
fn roundtrip_serialized_value_matches_struct_shape() {
    let raw = known_good_manifest_json().to_string();
    let manifest = PluginManifest::from_json(&raw).expect("parse");
    let as_value: Value = serde_json::to_value(&manifest).expect("to_value");
    let from_value: PluginManifest =
        serde_json::from_value(as_value.clone()).expect("from_value");
    let again: Value = serde_json::to_value(&from_value).expect("to_value again");
    assert_eq!(as_value, again);
    assert_manifests_equal(&manifest, &from_value);
}

/// Minimal required-only manifest roundtrips and fills defaults.
#[test]
fn roundtrip_minimal_manifest_applies_defaults() {
    let raw = json!({
        "id": "com.example.minimal",
        "name": "Minimal",
        "version": "0.1.0",
        "capabilities": ["tool"]
    })
    .to_string();
    let m = PluginManifest::from_json(&raw).expect("minimal parse");
    assert!(m.permissions.network.is_empty());
    assert!(!m.permissions.shell);
    assert_eq!(m.resources.max_fuel, PluginResourceConfig::default().max_fuel);
    assert_eq!(
        m.resources.max_memory_mb,
        PluginResourceConfig::default().max_memory_mb
    );
    assert!(m.wasm_module.is_none());
    assert!(m.skills.is_empty());
    assert!(m.tools.is_empty());
    assert!(m.voice.is_none());

    let hop1 = serde_json::to_string(&m).unwrap();
    let m2 = PluginManifest::from_json(&hop1).unwrap();
    assert_manifests_equal(&m, &m2);
    let hop2 = serde_json::to_string(&m2).unwrap();
    assert_eq!(hop1, hop2);
}

/// Every `PluginCapability` variant roundtrips through JSON snake_case.
#[test]
fn roundtrip_all_capability_variants() {
    let caps = [
        (PluginCapability::Tool, "tool"),
        (PluginCapability::Channel, "channel"),
        (PluginCapability::PipelineStage, "pipeline_stage"),
        (PluginCapability::Skill, "skill"),
        (PluginCapability::MemoryBackend, "memory_backend"),
        (PluginCapability::Voice, "voice"),
    ];
    for (cap, wire) in caps {
        let json = serde_json::to_string(&cap).unwrap();
        assert_eq!(json, format!("\"{wire}\""));
        let restored: PluginCapability = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, cap);
    }
}

// ── Forward-compat / version-compat ─────────────────────────────────────

/// Future / unknown top-level fields are ignored (serde default).
/// A producer may ship a newer schema without breaking older loaders.
#[test]
fn forward_compat_unknown_top_level_fields_ignored() {
    let mut body = known_good_manifest_json();
    let obj = body.as_object_mut().unwrap();
    obj.insert("schema_version".into(), json!(2));
    obj.insert("future_feature_flag".into(), json!(true));
    obj.insert(
        "experimental".into(),
        json!({"nested": "value", "list": [1, 2, 3]}),
    );

    let m = PluginManifest::from_json(&body.to_string())
        .expect("unknown fields must not break parse");
    assert_eq!(m.id, "com.example.test-plugin");
    assert_eq!(m.version, "1.2.3");
    // Known fields still intact.
    assert_eq!(m.tools, vec!["lint_code"]);
}

/// Explicit future `schema_version` (or similar) is ignored today —
/// there is no hard schema pin on `PluginManifest`; versioning is the
/// plugin's own semver `version` field.
#[test]
fn forward_compat_schema_version_field_ignored_gracefully() {
    for schema_version in [json!(1), json!(99), json!("2.0"), json!(null)] {
        let raw = json!({
            "id": "com.example.schema-v",
            "name": "Schema V",
            "version": "1.0.0",
            "capabilities": ["tool"],
            "schema_version": schema_version,
        })
        .to_string();
        let m = PluginManifest::from_json(&raw)
            .expect("schema_version must be ignored, not rejected");
        assert_eq!(m.id, "com.example.schema-v");
        assert_eq!(m.version, "1.0.0");
    }
}

/// Unknown keys under nested objects (permissions / resources / voice)
/// are also ignored.
#[test]
fn forward_compat_unknown_nested_fields_ignored() {
    let raw = json!({
        "id": "com.example.nested-future",
        "name": "Nested Future",
        "version": "0.3.0",
        "capabilities": ["voice"],
        "permissions": {
            "network": ["*"],
            "shell": true,
            "future_perm": "quantum"
        },
        "resources": {
            "max_fuel": 42u64,
            "gpu_quota": 7
        },
        "voice": {
            "read_transcripts": true,
            "new_sub_permission": true
        }
    })
    .to_string();
    let m = PluginManifest::from_json(&raw).expect("nested unknown ok");
    assert_eq!(m.permissions.network, vec!["*"]);
    assert!(m.permissions.shell);
    assert_eq!(m.resources.max_fuel, 42);
    // Defaults still applied for omitted resource keys.
    assert_eq!(
        m.resources.max_memory_mb,
        PluginResourceConfig::default().max_memory_mb
    );
    let voice = m.voice.expect("voice");
    assert!(voice.read_transcripts);
    assert!(!voice.dispatch_commands);
}

/// Semver prerelease / build metadata is accepted (valid version string).
#[test]
fn version_compat_semver_prerelease_and_build_accepted() {
    for version in ["1.0.0-alpha.1", "2.0.0-rc.1+build.42", "0.0.1+meta"] {
        let raw = json!({
            "id": "com.example.semver",
            "name": "Semver",
            "version": version,
            "capabilities": ["tool"]
        })
        .to_string();
        let m = PluginManifest::from_json(&raw)
            .unwrap_or_else(|e| panic!("version {version} should parse: {e}"));
        assert_eq!(m.version, version);
    }
}

/// Voice camelCase aliases remain accepted (version/compat surface).
#[test]
fn version_compat_voice_camel_case_aliases() {
    let raw = json!({
        "id": "com.example.camel-voice",
        "name": "Camel Voice",
        "version": "1.0.0",
        "capabilities": ["voice"],
        "voice": {
            "readTranscripts": true,
            "dispatchCommands": true,
            "synthesizeAudio": false,
            "transcriptTopics": ["t.a"]
        }
    })
    .to_string();
    let m = PluginManifest::from_json(&raw).unwrap();
    let v = m.voice.as_ref().expect("voice");
    assert!(v.read_transcripts);
    assert!(v.dispatch_commands);
    assert!(!v.synthesize_audio);
    assert_eq!(v.transcript_topics, vec!["t.a"]);

    // Roundtrip re-emits snake_case (canonical serialize form).
    let hop = serde_json::to_value(&m).unwrap();
    let voice = hop.get("voice").unwrap();
    assert!(voice.get("read_transcripts").is_some());
    assert!(voice.get("readTranscripts").is_none());
}

// ── Negative tests ──────────────────────────────────────────────────────

#[test]
fn negative_malformed_json_rejected() {
    let cases = [
        "",
        "{",
        "null",
        "[]",
        "not json at all",
        r#"{"id": "x", "name": "y", "version": "1.0.0", "capabilities": ["tool""#,
    ];
    for raw in cases {
        let err = PluginManifest::from_json(raw).expect_err("must fail");
        // Malformed JSON surfaces as Serialization (serde_json error).
        assert!(
            matches!(err, PluginError::Serialization(_))
                || err.to_string().to_lowercase().contains("json")
                || err.to_string().to_lowercase().contains("eof")
                || err.to_string().to_lowercase().contains("expected")
                || err.to_string().to_lowercase().contains("invalid"),
            "unexpected error shape for {raw:?}: {err}"
        );
    }
}

#[test]
fn negative_missing_required_fields_rejected() {
    // Drop each required key in turn.
    let required = ["id", "name", "version", "capabilities"];
    for drop_key in required {
        let mut body = json!({
            "id": "com.example.x",
            "name": "X",
            "version": "1.0.0",
            "capabilities": ["tool"]
        });
        body.as_object_mut().unwrap().remove(drop_key);
        let err = PluginManifest::from_json(&body.to_string())
            .expect_err(&format!("missing {drop_key} must fail"));
        let msg = err.to_string().to_lowercase();
        assert!(
            matches!(err, PluginError::Serialization(_))
                || msg.contains(drop_key)
                || msg.contains("missing")
                || msg.contains("expected"),
            "missing {drop_key}: got {err}"
        );
    }
}

#[test]
fn negative_empty_id_rejected() {
    let raw = json!({
        "id": "",
        "name": "X",
        "version": "1.0.0",
        "capabilities": ["tool"]
    })
    .to_string();
    let err = PluginManifest::from_json(&raw).unwrap_err();
    assert!(
        err.to_string().contains("id is required"),
        "got: {err}"
    );
}

#[test]
fn negative_empty_name_rejected() {
    let raw = json!({
        "id": "com.example.x",
        "name": "",
        "version": "1.0.0",
        "capabilities": ["tool"]
    })
    .to_string();
    let err = PluginManifest::from_json(&raw).unwrap_err();
    assert!(
        err.to_string().contains("name is required"),
        "got: {err}"
    );
}

#[test]
fn negative_empty_capabilities_rejected() {
    let raw = json!({
        "id": "com.example.x",
        "name": "X",
        "version": "1.0.0",
        "capabilities": []
    })
    .to_string();
    let err = PluginManifest::from_json(&raw).unwrap_err();
    assert!(
        err.to_string().contains("at least one capability"),
        "got: {err}"
    );
}

/// Wrong / invalid schema-level version: plugin `version` must be semver.
#[test]
fn negative_wrong_version_not_semver_rejected() {
    for bad in ["", "not-semver", "1", "1.0", "v1.0.0", "latest", "01.02.03"] {
        // Note: "1.0" is invalid for strict semver (needs MAJOR.MINOR.PATCH).
        // "01.02.03" may or may not parse depending on semver crate rules —
        // we only assert the clearly invalid ones below via expect_err for
        // the non-semver strings; for edge cases we document behaviour.
        let raw = json!({
            "id": "com.example.badver",
            "name": "BadVer",
            "version": bad,
            "capabilities": ["tool"]
        })
        .to_string();
        match PluginManifest::from_json(&raw) {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("invalid semver") || msg.contains("version"),
                    "bad version {bad:?}: {msg}"
                );
            }
            Ok(_) => {
                // Only allow success if the crate's semver parser accepts it.
                // Guard against accidental acceptance of clearly non-semver.
                assert!(
                    bad != "not-semver"
                        && !bad.is_empty()
                        && bad != "latest"
                        && bad != "v1.0.0"
                        && bad != "1"
                        && bad != "1.0",
                    "version {bad:?} must be rejected"
                );
            }
        }
    }
}

#[test]
fn negative_invalid_id_characters_rejected() {
    let raw = json!({
        "id": "com.example/bad id!",
        "name": "X",
        "version": "1.0.0",
        "capabilities": ["tool"]
    })
    .to_string();
    let err = PluginManifest::from_json(&raw).unwrap_err();
    assert!(
        err.to_string().contains("alphanumeric") || err.to_string().contains("id"),
        "got: {err}"
    );
}

#[test]
fn negative_unknown_capability_variant_rejected() {
    let raw = json!({
        "id": "com.example.cap",
        "name": "Cap",
        "version": "1.0.0",
        "capabilities": ["not_a_real_capability"]
    })
    .to_string();
    let err = PluginManifest::from_json(&raw).unwrap_err();
    // Serde enum reject — Serialization path.
    assert!(
        matches!(err, PluginError::Serialization(_))
            || err.to_string().to_lowercase().contains("unknown")
            || err.to_string().to_lowercase().contains("variant")
            || err.to_string().to_lowercase().contains("capability"),
        "got: {err}"
    );
}

#[test]
fn negative_wrong_types_for_required_fields() {
    let cases = [
        json!({
            "id": 123,
            "name": "X",
            "version": "1.0.0",
            "capabilities": ["tool"]
        }),
        json!({
            "id": "com.example.x",
            "name": ["not", "a", "string"],
            "version": "1.0.0",
            "capabilities": ["tool"]
        }),
        json!({
            "id": "com.example.x",
            "name": "X",
            "version": 1.0,
            "capabilities": ["tool"]
        }),
        json!({
            "id": "com.example.x",
            "name": "X",
            "version": "1.0.0",
            "capabilities": "tool"
        }),
    ];
    for body in cases {
        let err = PluginManifest::from_json(&body.to_string()).expect_err("type mismatch");
        assert!(
            matches!(err, PluginError::Serialization(_)),
            "expected Serialization for {body}, got {err}"
        );
    }
}

// ── Permissions / resources structural checks ───────────────────────────

#[test]
fn permissions_and_resources_roundtrip_independently() {
    let perms = PluginPermissions {
        network: vec!["*.example.com".into(), "api.test.com".into()],
        filesystem: vec!["/tmp".into()],
        env_vars: vec!["KEY".into()],
        shell: true,
    };
    let p_json = serde_json::to_string(&perms).unwrap();
    let p2: PluginPermissions = serde_json::from_str(&p_json).unwrap();
    assert_eq!(p2, perms);

    let resources = PluginResourceConfig {
        max_fuel: 99,
        max_memory_mb: 4,
        max_http_requests_per_minute: 3,
        max_log_messages_per_minute: 7,
        max_execution_seconds: 11,
        max_table_elements: 13,
    };
    let r_json = serde_json::to_string(&resources).unwrap();
    let r2: PluginResourceConfig = serde_json::from_str(&r_json).unwrap();
    assert_eq!(r2.max_fuel, resources.max_fuel);
    assert_eq!(r2.max_memory_mb, resources.max_memory_mb);
    assert_eq!(
        r2.max_http_requests_per_minute,
        resources.max_http_requests_per_minute
    );
    assert_eq!(
        r2.max_log_messages_per_minute,
        resources.max_log_messages_per_minute
    );
    assert_eq!(r2.max_execution_seconds, resources.max_execution_seconds);
    assert_eq!(r2.max_table_elements, resources.max_table_elements);
}

#[test]
fn voice_capability_roundtrip() {
    let v = VoiceCapability {
        read_transcripts: true,
        dispatch_commands: false,
        synthesize_audio: true,
        transcript_topics: vec!["t.a".into(), "t.b".into()],
    };
    let json = serde_json::to_string(&v).unwrap();
    let restored: VoiceCapability = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, v);
    let hop2 = serde_json::to_string(&restored).unwrap();
    assert_eq!(json, hop2);
}
