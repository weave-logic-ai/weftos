//! The install flow's checks (ADR-107): before installing a sensor cog on a node, and after.
//!
//! Pre-install turns the host's `/hw/buses` facts plus the module's bus and the cog's marketplace
//! entry into pass / fail / warn / verify-by-hand rows with the fix to apply. It claims only what
//! the host actually reported: a device node that exists, the kernel command line, the cogs
//! already enabled. Everything else is "verify by hand", pointing at the guide. Post-install reads
//! the cog's own last output line (`health`, `source.verified`). Egui-independent.

use crate::client::NodeFacts;
use crate::sensor_link::{detect_bus, Bus, CogOutput, CogState, Installed};
use weftos_cog_market::hw::{HwCatalog, Module};
use weftos_cog_market::CatalogItem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    /// Probably a problem, but not certain.
    Warn,
    /// The host cannot check this; do it by hand (the fix says how).
    Manual,
}

impl Verdict {
    pub fn mark(self) -> &'static str {
        match self {
            Verdict::Pass => "✔",
            Verdict::Fail => "✖",
            Verdict::Warn => "!",
            Verdict::Manual => "?",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub verdict: Verdict,
    pub detail: String,
    /// What to do about it (empty on a pass).
    pub fix: String,
}

fn check(name: &'static str, verdict: Verdict, detail: impl Into<String>, fix: impl Into<String>) -> Check {
    Check { name, verdict, detail: detail.into(), fix: fix.into() }
}

/// True when no check failed outright (warnings and manual rows do not block).
pub fn ready(checks: &[Check]) -> bool {
    checks.iter().all(|c| c.verdict != Verdict::Fail)
}

/// Marketplace arch name for the arch Rust reports on the host (`std::env::consts::ARCH`).
pub fn marketplace_arch(host_arch: &str) -> &str {
    match host_arch {
        "arm" => "arm",
        "aarch64" => "arm64",
        other => other,
    }
}

pub fn preinstall(m: &Module, cog_id: &str, item: Option<&CatalogItem>, facts: Option<&NodeFacts>, cat: &HwCatalog) -> Vec<Check> {
    let Some(f) = facts else {
        return vec![check("node facts", Verdict::Manual, "the node's bus facts are not available", "verify by hand with the guide's setup and wiring pages")];
    };
    let mut v = Vec::new();
    v.push(match item {
        Some(i) if i.arches.iter().any(|a| a == marketplace_arch(&f.arch)) => check("architecture", Verdict::Pass, format!("{} build available for {}", marketplace_arch(&f.arch), f.arch), ""),
        Some(i) => check("architecture", Verdict::Fail, format!("node is {}, the cog is built for {}", f.arch, i.arches.join("/")), "install on a node of a supported architecture"),
        None => check("architecture", Verdict::Manual, format!("node is {}; no marketplace build to compare", f.arch), "build the cog for this architecture (sensor-cog skill)"),
    });
    let bus = detect_bus(m);
    match bus {
        Bus::Uart => {
            let have = !f.uart.devices.is_empty() || !f.usb_serial.is_empty();
            let found: Vec<&str> = f.uart.devices.iter().chain(&f.usb_serial).map(String::as_str).collect();
            v.push(if have {
                check("UART device", Verdict::Pass, format!("found {}", found.join(", ")), "")
            } else {
                check("UART device", Verdict::Fail, "no /dev/serial0, /dev/ttyAMA0, /dev/ttyUSB* or /dev/ttyACM* on the node", "enable the hardware UART (needs the owner's approval) or plug in a USB-serial adapter; see the guide's setup page")
            });
            if !f.uart.devices.is_empty() {
                v.push(match f.uart.console_on_uart {
                    Some(true) => check("serial console", Verdict::Fail, format!("the kernel console is on the UART ({})", f.uart.console.clone().unwrap_or_default()), "remove that console= entry from the kernel command line and reboot (needs the owner's approval)"),
                    Some(false) => check("serial console", Verdict::Pass, "the kernel console is not on the UART", ""),
                    None => check("serial console", Verdict::Manual, "the kernel command line was not readable", "check /boot/firmware/cmdline.txt has no console=serial0 / ttyAMA0"),
                });
            }
        }
        Bus::I2c | Bus::Analog => {
            v.push(if f.i2c.devices.is_empty() {
                check("I2C bus", Verdict::Fail, "no /dev/i2c-* on the node", "enable I2C (needs the owner's approval), then reboot; see the guide's setup page")
            } else {
                check("I2C bus", Verdict::Pass, format!("found {}", f.i2c.devices.join(", ")), "")
            });
            v.push(check("I2C address", Verdict::Manual, "the host cannot see what is wired", "run `i2cdetect -y 1` with the sensor connected and compare with the guide"));
        }
        Bus::Usb => v.push(if f.usb_serial.is_empty() {
            check("USB serial device", Verdict::Fail, "no /dev/ttyUSB* or /dev/ttyACM* on the node", "plug the module in; check the cable carries data")
        } else {
            check("USB serial device", Verdict::Pass, format!("found {}", f.usb_serial.join(", ")), "")
        }),
        Bus::Unknown => v.push(check("bus", Verdict::Manual, "the catalog does not say which bus this module uses", "follow the guide's wiring page")),
    }
    // Another enabled cog on the same kind of bus.
    if bus == Bus::Uart {
        let others: Vec<&str> = f
            .enabled_cogs
            .iter()
            .filter(|c| c.id != cog_id)
            .filter(|c| cat.modules_for_cog(&c.id).iter().any(|m| detect_bus(m) == Bus::Uart))
            .map(|c| c.id.as_str())
            .collect();
        if !others.is_empty() {
            v.push(check("port conflict", Verdict::Warn, format!("{} also reads a UART on this node", others.join(", ")), "two cogs cannot share one serial port: point one at a USB-serial adapter, or stop the other"));
        }
    }
    v.push(match item {
        Some(i) if i.signed => check("provenance", Verdict::Pass, "signed WeaveLogic cog; the host verifies it at install", ""),
        Some(_) => check("licence / provenance", Verdict::Manual, "unsigned Cognitum cog; the host checks its sha256 and licence at install", "if the host refuses it, the console shows the reason"),
        None => check("licence / provenance", Verdict::Manual, "not in any configured marketplace source", "stage it with `weft-cog-host add` (local source)"),
    });
    v
}

/// After install: is it up, and does it see the sensor?
pub fn postinstall(installed: Option<&Installed>, out: Option<&CogOutput>) -> Vec<Check> {
    let Some(i) = installed else {
        return vec![check("installed", Verdict::Manual, "not installed on the connected node", "install it first")];
    };
    let mut v = vec![match (i.state, &i.refusal) {
        (CogState::Running, _) => check("running", Verdict::Pass, format!("running v{}", i.version), ""),
        (_, Some(code)) => check("running", Verdict::Fail, format!("the host refused to run it ({code})"), "fix the licence grant, then start it"),
        (s, _) => check("running", Verdict::Fail, s.label(), "start it, then watch its output"),
    }];
    if i.state != CogState::Running {
        return v;
    }
    v.push(match out {
        None => check("sensor source", Verdict::Manual, "no output line yet", "give it a few seconds, then reopen; or read its log"),
        Some(o) if o.health.as_deref() == Some("no_source") => check("sensor source", Verdict::Fail, format!("the cog reports no_source{}", reasons(o)), "nothing readable on its device: check power, the TX/RX crossing, the device path and baud (guide: wiring and troubleshoot)"),
        Some(o) if o.simulated => check("sensor source", Verdict::Warn, "the cog is running in simulate mode, not on the real sensor", "turn simulate off in its config"),
        Some(o) if o.health.as_deref() == Some("degraded") => check("sensor source", Verdict::Warn, format!("sensor found, link degraded{}", reasons(o)), "see the guide's troubleshoot page"),
        Some(o) if o.verified == Some(true) || o.health.as_deref() == Some("ok") => check("sensor source", Verdict::Pass, format!("sensor found{}", o.frame_rate_hz.map(|f| format!(", {f:.1} frames/s")).unwrap_or_default()), ""),
        Some(o) => check("sensor source", Verdict::Manual, format!("health {}", o.health.clone().unwrap_or_else(|| "unknown".into())), "compare with the guide's verify steps"),
    });
    v
}

fn reasons(o: &CogOutput) -> String {
    if o.reasons.is_empty() {
        String::new()
    } else {
        format!(" ({})", o.reasons.join(", "))
    }
}

/// `http://<ip>:<port>` of a peer's cog-host, reusing the port of the host the console is on.
/// `None` unless `ip` is a literal tailnet address (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`): the
/// console never offers to send a host token to an address it did not get from the tailnet.
pub fn peer_url(connected_host: &str, ip: &str) -> Option<String> {
    let addr = weftos_cog_market::net::tailnet_ip(ip)?;
    let rest = connected_host.trim().trim_end_matches('/');
    let rest = rest.strip_prefix("http://").or_else(|| rest.strip_prefix("https://")).unwrap_or(rest);
    let port = rest.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).unwrap_or(9480);
    Some(match addr {
        std::net::IpAddr::V4(a) => format!("http://{a}:{port}"),
        std::net::IpAddr::V6(a) => format!("http://[{a}]:{port}"),
    })
}

/// Per-host tokens when the console moves from one host to another: remember the token of the host
/// being left, and return the token for the new one (what was entered for it before, else empty).
/// The old host's token is never returned for a different host.
pub fn switch_token(tokens: &mut std::collections::BTreeMap<String, String>, from_host: &str, from_token: &str, to_host: &str) -> String {
    tokens.insert(from_host.trim().to_string(), from_token.to_string());
    if from_host.trim() == to_host.trim() {
        return from_token.to_string();
    }
    tokens.get(to_host.trim()).cloned().unwrap_or_default()
}

/// True when the console points at this machine, which is rarely what a user on a remote node meant.
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim().trim_start_matches("http://").trim_start_matches("https://");
    let name = h.split(['/', ':']).next().unwrap_or("");
    name == "localhost" || name == "127.0.0.1" || name == "::1" || name == "[::1]"
}

#[cfg(test)]
#[path = "sensor_install_tests.rs"]
mod tests;
