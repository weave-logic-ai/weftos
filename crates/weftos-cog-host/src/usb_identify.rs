//! "Ask agent" for the Identify-hardware modal: build a prompt from one USB device (never its full
//! serial) and run an operator-configured agent command to answer it. std-only, hard timeout, capped
//! output. The command comes from `WEFT_COG_HOST_AGENT_CMD` (shell-split argv, prompt appended as the
//! last argument), else `weft agent -m <prompt>` when `weft` is on PATH.

use crate::usb::{id_json, UsbDevice};
use serde_json::{json, Value};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use weftos_cog_market::usb::UsbIdTable;

pub const AGENT_ENV: &str = "WEFT_COG_HOST_AGENT_CMD";
pub const AGENT_TIMEOUT: Duration = Duration::from_secs(90);
pub const MAX_OUTPUT: usize = 16 * 1024;

/// One identify at a time: the agent may be a paid or local LLM, and the call blocks a thread.
static BUSY: AtomicBool = AtomicBool::new(false);

/// Prompt describing the device. Only the vid/pid, strings, class, speed, ports and id-table hint.
pub fn build_prompt(d: &UsbDevice, table: &UsbIdTable) -> String {
    let mut p = String::from(
        "Identify this USB device attached to a WeftOS appliance. Say what it most likely is (board/chip/adapter), \
         what it is typically used for, and any driver, flashing or serial-port tips. Be concise (under 150 words).\n\n",
    );
    p += &format!("vid:pid = {:04x}:{:04x}\n", d.vid, d.pid);
    for (k, v) in [("manufacturer", &d.manufacturer), ("product", &d.product), ("class", &d.class), ("speed", &d.speed)] {
        if !v.is_empty() {
            p += &format!("{k}: {v}\n");
        }
    }
    if !d.ports.is_empty() {
        p += &format!("serial ports: {}\n", d.ports.join(", "));
    }
    if let Some(i) = table.lookup_device(d.vid, d.pid, &d.product) {
        p += &format!("id-table hint: {} ({})", i.name, i.kind);
        if !i.notes.is_empty() {
            p += &format!(" - {}", i.notes);
        }
        p.push('\n');
    }
    p
}

/// Minimal shell-style split: whitespace separated, `'...'` and `"..."` group, backslash escapes.
pub fn shell_split(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut in_tok) = (Vec::new(), String::new(), false);
    let mut quote: Option<char> = None;
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                if let Some(n) = it.next() {
                    cur.push(n);
                    in_tok = true;
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_tok = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_tok {
                    out.push(std::mem::take(&mut cur));
                    in_tok = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_tok = true;
            }
        }
    }
    if in_tok {
        out.push(cur);
    }
    out
}

/// Pick the agent argv (without the prompt): env override, else `weft agent -m` when installed.
pub fn agent_argv(env: Option<&str>, weft_on_path: bool) -> Option<Vec<String>> {
    if let Some(e) = env.map(shell_split).filter(|v| !v.is_empty()) {
        return Some(e);
    }
    weft_on_path.then(|| vec!["weft".into(), "agent".into(), "-m".into()])
}

fn weft_on_path() -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("weft").is_file()))
}

/// Run `argv + [prompt]`, kill it after `timeout`, keep at most `cap` bytes of stdout.
pub fn run_command(argv: &[String], prompt: &str, timeout: Duration, cap: usize) -> Result<String, String> {
    let (prog, rest) = argv.split_first().ok_or("empty agent command")?;
    let mut child = Command::new(prog)
        .args(rest)
        .arg(prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {prog}: {e}"))?;
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut out = child.stdout.take().ok_or("no stdout")?;
    let sink = Arc::clone(&buf);
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = out.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let mut b = sink.lock().unwrap();
            let room = cap.saturating_sub(b.len());
            b.extend_from_slice(&chunk[..n.min(room)]);
        }
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("agent timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(format!("wait: {e}")),
        }
    };
    std::thread::sleep(Duration::from_millis(30)); // let the reader drain the pipe tail
    let text = String::from_utf8_lossy(&buf.lock().unwrap()).trim().to_string();
    if !status.success() && text.is_empty() {
        return Err(format!("agent exited with {status}"));
    }
    if text.is_empty() {
        return Err("agent returned no output".into());
    }
    Ok(text)
}

fn unavailable() -> Value {
    json!({ "ok": false, "error": "no agent available", "hint": format!("set {AGENT_ENV}") })
}

/// Body for `POST /hw/usb/identify`: `Ok` JSON or an error JSON (status picked by the caller).
pub fn identify(dev: &UsbDevice, table: &UsbIdTable) -> (bool, Value) {
    identify_with(dev, table, agent_argv(std::env::var(AGENT_ENV).ok().as_deref(), weft_on_path()), AGENT_TIMEOUT)
}

pub fn identify_with(dev: &UsbDevice, table: &UsbIdTable, argv: Option<Vec<String>>, timeout: Duration) -> (bool, Value) {
    let Some(argv) = argv else { return (false, unavailable()) };
    if BUSY.swap(true, Ordering::AcqRel) {
        return (false, json!({ "ok": false, "error": "an identify request is already running" }));
    }
    let res = run_command(&argv, &build_prompt(dev, table), timeout, MAX_OUTPUT);
    BUSY.store(false, Ordering::Release);
    match res {
        Ok(answer) => (true, json!({ "ok": true, "key": dev.key, "answer": answer, "id": id_json(table, dev) })),
        Err(e) => (false, json!({ "ok": false, "error": e })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // identify_with shares the global BUSY flag; serialise the tests that go through it.
    static LOCK: StdMutex<()> = StdMutex::new(());

    fn dev() -> UsbDevice {
        UsbDevice {
            key: "10c4:ea60:s-deadbeef".into(),
            vid: 0x10c4,
            pid: 0xea60,
            product: "CP2102".into(),
            serial: Some("SECRETSERIAL999".into()),
            ports: vec!["/dev/ttyUSB0".into()],
            ..Default::default()
        }
    }

    #[test]
    fn prompt_has_hints_but_never_the_serial() {
        let p = build_prompt(&dev(), &UsbIdTable::bundled());
        assert!(p.contains("10c4:ea60") && p.contains("CP2102") && p.contains("/dev/ttyUSB0"));
        assert!(p.contains("id-table hint: Silicon Labs CP210x"));
        assert!(!p.contains("SECRETSERIAL999") && !p.contains("999"));
    }

    #[test]
    fn shell_split_handles_quotes() {
        assert_eq!(shell_split(r#"claude -p "be brief" 'x y'"#), vec!["claude", "-p", "be brief", "x y"]);
        assert!(shell_split("   ").is_empty());
    }

    #[test]
    fn argv_selection() {
        assert_eq!(agent_argv(Some("echo hi"), true).unwrap(), vec!["echo", "hi"]);
        assert_eq!(agent_argv(None, true).unwrap(), vec!["weft", "agent", "-m"]);
        assert!(agent_argv(None, false).is_none());
        assert!(agent_argv(Some("  "), false).is_none());
    }

    #[test]
    fn unavailable_agent_reports_hint() {
        let _g = LOCK.lock().unwrap();
        let (ok, v) = identify_with(&dev(), &UsbIdTable::bundled(), None, Duration::from_secs(1));
        assert!(!ok);
        assert_eq!(v["error"], "no agent available");
        assert!(v["hint"].as_str().unwrap().contains(AGENT_ENV));
    }

    #[test]
    fn echo_agent_returns_answer_with_prompt_as_last_arg() {
        let _g = LOCK.lock().unwrap();
        let (ok, v) = identify_with(&dev(), &UsbIdTable::bundled(), Some(vec!["echo".into()]), Duration::from_secs(5));
        assert!(ok, "{v}");
        assert!(v["answer"].as_str().unwrap().contains("10c4:ea60"));
    }

    #[test]
    fn slow_agent_is_killed_at_timeout() {
        let _g = LOCK.lock().unwrap();
        let t = Instant::now();
        // `sleep <prompt>` fails fast on a non-number; use sh -c so the prompt becomes $0 and sleep runs.
        let argv = vec!["sh".into(), "-c".into(), "sleep 30".into()];
        let (ok, v) = identify_with(&dev(), &UsbIdTable::bundled(), Some(argv), Duration::from_millis(300));
        assert!(!ok);
        assert!(v["error"].as_str().unwrap().contains("timed out"), "{v}");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn output_is_capped() {
        let argv = vec!["sh".into(), "-c".into(), "yes x | head -c 200000".into()];
        let out = run_command(&argv, "p", Duration::from_secs(5), 1000).unwrap();
        assert!(out.len() <= 1000);
    }
}
