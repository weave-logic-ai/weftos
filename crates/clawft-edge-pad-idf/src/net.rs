//! WiFi bring-up via esp-idf-svc — STA, DHCP, blocking-wait for link.
//!
//! Pattern mirrors `crates/clawft-edge-bench/src/main.rs::connect_wifi`:
//! `BlockingWifi::wrap(EspWifi::new(modem, sysloop, nvs))`, configure
//! as STA, start, connect, wait for netif up.
//!
//! The bare-metal port at `clawft-edge-pad/src/net.rs` used esp-radio +
//! embassy-net (no_std) — that whole machine is replaced here by one
//! ESP-IDF call tree. Configuration values + retry semantics come from
//! the canonical IDF-on-Rust example in `clawft-edge-bench`.

use anyhow::Result;
use esp_idf_hal::modem::Modem;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use log::info;

use crate::wifi_secrets;

/// Connect to the configured AP and return the wrapped wifi handle.
/// Caller keeps the handle alive — dropping it tears down the link.
///
/// `Modem<'static>` is required by `EspWifi::new`'s lifetime bound:
/// the WiFi driver holds the modem peripheral for its full lifetime,
/// which we want to be `'static` so the returned handle is `'static`.
/// The `Peripherals` singleton in `main.rs` gives us the `'static`-
/// scoped modem.
pub fn connect_wifi(
    modem: Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
) -> Result<BlockingWifi<EspWifi<'static>>> {
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sysloop.clone(), Some(nvs))?,
        sysloop,
    )?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: wifi_secrets::WIFI_SSID
            .try_into()
            .expect("ssid <= 32 chars"),
        password: wifi_secrets::WIFI_PASSWORD
            .try_into()
            .expect("password <= 64 chars"),
        ..Default::default()
    }))?;

    info!("[net] starting wifi radio");
    wifi.start()?;
    info!("[net] associating with '{}'", wifi_secrets::WIFI_SSID);
    wifi.connect()?;
    wifi.wait_netif_up()?;

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
    info!("[net] online — ip={} gw={}", ip_info.ip, ip_info.subnet.gateway);
    Ok(wifi)
}

// Note on retries: `EspWifi::new` consumes the `Modem` peripheral, and
// the peripheral singleton cannot be re-taken. The bare-metal port's
// reconnect-on-disconnect loop is served by esp-idf-svc internally:
// once `wifi.connect()` succeeds, the IDF WiFi stack auto-reconnects
// on link loss. A retry around the initial call would require
// reboot-on-fail semantics, which the firmware caller (`main.rs`)
// already provides by holding the failure path open.
