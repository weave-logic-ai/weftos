//! Request sanitiser for the TCP relay (ADR-102 D3, Phase 0 review R3).
//!
//! The relay used to byte-copy TCP into the unix socket, so a TCP caller
//! that sent `"auth":"admin"` got the local-owner shortcut. The relay now
//! reads the TCP side line by line and drops any `auth` that is a literal
//! scope string (`admin`, `write,chat`, ...), so a relayed caller is
//! anonymous unless it presents a token secret (`wft_...`), which the
//! daemon validates. Binary RVF framing is refused over TCP because its
//! frames cannot be inspected here.
//!
//! The daemon-to-client direction is copied unchanged.

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UnixStream};

use crate::capability::is_literal_scope;

/// Longest request line the relay forwards.
pub const MAX_LINE: u64 = 4 * 1024 * 1024;

/// Sanitise one remote request line.
///
/// * A self-asserted literal scope in `auth` is dropped (lines with no
///   string `auth`, or a token secret, keep their `auth`).
/// * `forward` (the user-signed proxy header) and `project` are always
///   dropped: a forward header is honoured only from local unix callers, and
///   a remote caller's project claim means nothing to the local daemon.
///
/// Lines that are not a JSON object, and objects with nothing to drop, are
/// returned unchanged.
pub fn sanitize_line(line: &[u8]) -> Vec<u8> {
    let Ok(mut v) = serde_json::from_slice::<Value>(line) else {
        return line.to_vec();
    };
    let Some(o) = v.as_object_mut() else {
        return line.to_vec();
    };
    let literal = o.get("auth").and_then(Value::as_str).is_some_and(is_literal_scope);
    let mut changed = false;
    if literal {
        o.remove("auth");
        changed = true;
    }
    for k in ["forward", "project"] {
        changed |= o.remove(k).is_some();
    }
    if !changed {
        return line.to_vec();
    }
    let mut out = serde_json::to_vec(&v).unwrap_or_else(|_| line.to_vec());
    out.push(b'\n');
    out
}

/// Pump `tcp` to `unix` through [`sanitize_line`]. Returns bytes
/// (client-to-daemon, daemon-to-client).
pub async fn relay(tcp: TcpStream, unix: UnixStream) -> std::io::Result<(u64, u64)> {
    let (tcp_r, mut tcp_w) = tcp.into_split();
    let (mut unix_r, mut unix_w) = unix.into_split();
    let mut reader = BufReader::new(tcp_r);

    let first = reader.fill_buf().await?;
    if first.first() == Some(&b'R') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "binary RVF framing is not accepted over the TCP relay",
        ));
    }

    let up = async {
        let mut sent = 0u64;
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let n = (&mut reader)
                .take(MAX_LINE)
                .read_until(b'\n', &mut buf)
                .await?;
            if n == 0 || (!buf.ends_with(b"\n") && n as u64 >= MAX_LINE) {
                break;
            }
            let out = sanitize_line(&buf);
            unix_w.write_all(&out).await?;
            sent += out.len() as u64;
        }
        unix_w.shutdown().await.ok();
        Ok::<u64, std::io::Error>(sent)
    };
    let down = tokio::io::copy(&mut unix_r, &mut tcp_w);
    tokio::pin!(up, down);
    let (mut a, mut b) = (0, 0);
    tokio::select! {
        r = &mut up => a = r?,
        r = &mut down => b = r?,
    }
    Ok((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth_of(line: &[u8]) -> Option<String> {
        let v: Value = serde_json::from_slice(line).unwrap();
        v.get("auth").and_then(Value::as_str).map(str::to_owned)
    }

    #[test]
    fn literal_scopes_are_stripped() {
        for lit in ["admin", "write", "write,chat", "read, chat"] {
            let line = format!(r#"{{"method":"kernel.shutdown","auth":"{lit}","id":"1"}}"#);
            let out = sanitize_line(line.as_bytes());
            assert_eq!(auth_of(&out), None, "{lit}");
            let v: Value = serde_json::from_slice(&out).unwrap();
            assert_eq!(v["method"], "kernel.shutdown");
            assert!(out.ends_with(b"\n"));
        }
    }

    #[test]
    fn forward_and_project_never_cross_the_relay() {
        let line = br#"{"method":"agent.list","auth":"wft_abc","project":"01JB8Z3Q0V6X9KQ4M2N7T5R1WD","forward":{"project_id":"01JB8Z3Q0V6X9KQ4M2N7T5R1WD","issued_at_ms":1,"sig":"00"}}"#;
        let out = sanitize_line(line);
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v.get("forward").is_none());
        assert!(v.get("project").is_none());
        // The token and the rest of the request survive.
        assert_eq!(v["auth"], "wft_abc");
        assert_eq!(v["method"], "agent.list");
        // Also when a literal scope is stripped in the same pass.
        let both = br#"{"method":"x","auth":"admin","forward":{"sig":"00"}}"#;
        let v: Value = serde_json::from_slice(&sanitize_line(both)).unwrap();
        assert!(v.get("forward").is_none() && v.get("auth").is_none());
    }

    #[test]
    fn token_secrets_and_other_lines_pass_through() {
        let tok = br#"{"method":"x","auth":"wft_abc"}"#.to_vec();
        assert_eq!(sanitize_line(&tok), tok);
        let none = br#"{"method":"x"}"#.to_vec();
        assert_eq!(sanitize_line(&none), none);
        let junk = b"not json\n".to_vec();
        assert_eq!(sanitize_line(&junk), junk);
        let mixed = br#"{"method":"x","auth":"admin,wft_abc"}"#.to_vec();
        assert_eq!(sanitize_line(&mixed), mixed);
    }
}
