//! `weaver cluster nodes --facts`: print signed node facts with provenance
//! (card mesh-placement-03).
//!
//! Each entry's signed envelope is re-verified here, independently of the
//! daemon, before it is printed as verified.

use clawft_kernel::node_facts_advert::verify_node_facts;
use clawft_types::placement::{AttrValue, Capability};
use comfy_table::{Table, presets};

use crate::client::DaemonClient;
use crate::node_facts_rpc::FactsEntry;
use crate::protocol;

const BYTE_ATTRS: &[&str] = &["total", "free", "mem_bytes", "mem_free_bytes"];

fn gib(n: i64) -> String {
    format!("{:.1} GiB", n as f64 / (1u64 << 30) as f64)
}

/// Render one attribute value; byte-count attributes also get GiB.
pub fn fmt_attr(name: &str, v: &AttrValue) -> String {
    match v {
        AttrValue::Int(i) if BYTE_ATTRS.contains(&name) => format!("{name}={i} ({})", gib(*i)),
        AttrValue::Str(s) => format!("{name}={s:?}"),
        AttrValue::List(l) => {
            let items: Vec<String> = l
                .iter()
                .map(|x| match x {
                    AttrValue::Str(s) => s.clone(),
                    other => fmt_attr("", other).trim_start_matches('=').to_string(),
                })
                .collect();
            format!("{name}=[{}]", items.join(","))
        }
        AttrValue::Int(i) => format!("{name}={i}"),
        AttrValue::Float(f) => format!("{name}={f}"),
        AttrValue::Bool(b) => format!("{name}={b}"),
    }
}

fn row(c: &Capability) -> Vec<String> {
    let attrs: Vec<String> = c.attrs.iter().map(|(k, v)| fmt_attr(k, v)).collect();
    vec![
        c.id.to_string(),
        format!("{:?}", c.provenance).to_lowercase(),
        format!("{:?}", c.state).to_lowercase() + if c.exclusive { " excl" } else { "" },
        attrs.join(" "),
    ]
}

/// Render entries as text (pure; tested).
pub fn render(entries: &[FactsEntry], now: u64) -> String {
    if entries.is_empty() {
        return "No node facts cached.\n".into();
    }
    let mut out = String::new();
    for e in entries {
        let verified = match verify_node_facts(&e.signed, now) {
            Ok(_) => "verified (ed25519, node key)".to_string(),
            Err(err) => format!("NOT VERIFIED: {err}"),
        };
        out.push_str(&format!(
            "Node {}{}  trust={:?}  seq={}  issued_at={}  expires_in={}s\nSignature: {verified}\n",
            e.node_id,
            if e.local { " (local)" } else { "" },
            e.trust_tier,
            e.facts.seq,
            e.facts.issued_at,
            e.expires_at.saturating_sub(now),
        ));
        let mut table = Table::new();
        table.load_preset(presets::UTF8_FULL_CONDENSED);
        table.set_header(vec!["Capability", "Provenance", "State", "Attributes"]);
        for c in e.facts.capabilities() {
            table.add_row(row(c));
        }
        out.push_str(&format!("{table}\n"));
        if !e.facts.notes.is_empty() {
            out.push_str("Provenance notes:\n");
            for n in &e.facts.notes {
                out.push_str(&format!("  {}: {}\n", n.probe, n.note));
            }
        }
        out.push('\n');
    }
    out
}

/// Fetch `cluster.facts` and print it.
pub async fn run(
    client: &mut DaemonClient,
    refresh: bool,
    node: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let params = serde_json::json!({ "refresh": refresh, "node_id": node });
    let resp = client
        .call(protocol::Request::with_params("cluster.facts", params))
        .await?;
    if !resp.ok {
        anyhow::bail!(resp.error.unwrap_or_else(|| "unknown error".into()));
    }
    let value = resp.result.unwrap_or_default();
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    let entries: Vec<FactsEntry> = serde_json::from_value(value)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    print!("{}", render(&entries, now));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_kernel::node_facts_advert::sign_node_facts;
    use clawft_types::placement::{CapabilityId, NodeFacts, ProbeNote, Provenance, TrustTier};
    use ed25519_dalek::SigningKey;

    fn entry(tamper: bool) -> FactsEntry {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let id = clawft_kernel::node_id_from_pubkey(&key.verifying_key().to_bytes());
        let mut f = NodeFacts::new(id.clone(), 1_000, 600, 3);
        f.capabilities = vec![
            Capability::new(
                CapabilityId::new("mem.unified").unwrap(),
                Provenance::Probed,
            )
            .with_attr("total", 137_438_953_472i64),
            Capability::new(
                CapabilityId::new("accel.npu.ane").unwrap(),
                Provenance::Claimed,
            ),
        ];
        f.notes = vec![ProbeNote::new(
            "accel.npu.ane",
            "inferred from chip + CoreML",
        )];
        let mut signed = sign_node_facts(&f, &key).unwrap();
        if tamper {
            signed.payload = signed.payload.replace("claimed", "measured");
        }
        FactsEntry {
            node_id: id,
            local: true,
            trust_tier: TrustTier::Pinned,
            received_at: 1_000,
            expires_at: 1_600,
            delta_seq: 0,
            facts: f,
            signed,
        }
    }

    #[test]
    fn renders_provenance_memory_and_verification() {
        let text = render(&[entry(false)], 1_100);
        assert!(text.contains("(local)"));
        assert!(text.contains("Signature: verified"));
        assert!(text.contains("total=137438953472 (128.0 GiB)"));
        assert!(text.contains("claimed"));
        assert!(text.contains("accel.npu.ane: inferred from chip + CoreML"));
    }

    #[test]
    fn tampered_envelope_is_flagged() {
        let text = render(&[entry(true)], 1_100);
        assert!(text.contains("NOT VERIFIED"), "{text}");
    }
}
