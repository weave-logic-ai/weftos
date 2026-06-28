//! WiFi bring-up + embassy-net DHCP stack — edge-pad task #3.
//!
//! esp-radio 0.17 + embassy-net 0.9. The API shape is cribbed from
//! the `infinition/waveshare-watch-rs` reference firmware (same
//! ESP32-S3, same esp-radio / esp-rtos versions). `esp_radio::init()`
//! is argless because the `esp-radio` feature on `esp-rtos` (set in
//! Cargo.toml) wires the scheduler/timer integration.
//!
//! Credentials come from `wifi_secrets.rs` (gitignored).

use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_hal::peripherals::WIFI;
use esp_radio::wifi::{
    AuthMethod, ClientConfig, ModeConfig, WifiController, WifiDevice, WifiEvent,
};
use esp_println::println;
use static_cell::StaticCell;

use crate::wifi_secrets;

/// Drives the embassy-net stack. Must be spawned for any networking.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) -> ! {
    runner.run().await
}

/// Connects — and reconnects — to the configured AP.
#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) {
    println!(
        "[net] connection task — SSID '{}'",
        wifi_secrets::WIFI_SSID
    );
    loop {
        if matches!(controller.is_connected(), Ok(true)) {
            // Parked until the link drops.
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            println!("[net] wifi disconnected — retry in 3s");
            Timer::after(Duration::from_secs(3)).await;
        }
        if !matches!(controller.is_started(), Ok(true)) {
            match controller.start_async().await {
                Ok(()) => println!("[net] wifi radio started"),
                Err(e) => {
                    println!("[net] wifi start failed: {:?} — retry 2s", e);
                    Timer::after(Duration::from_secs(2)).await;
                    continue;
                }
            }
        }
        match controller.connect_async().await {
            Ok(()) => println!("[net] wifi connected"),
            Err(e) => {
                println!("[net] wifi connect failed: {:?} — retry 3s", e);
                Timer::after(Duration::from_secs(3)).await;
            }
        }
    }
}

/// Bring up WiFi + the embassy-net DHCP stack. Returns the `Stack`
/// handle (a `Copy` handle) — the mesh client (task #4) opens
/// sockets on it. The link comes up asynchronously; callers should
/// `stack.wait_link_up().await` / check `stack.config_v4()` before
/// using it.
pub fn start(spawner: &Spawner, wifi: WIFI<'static>) -> Stack<'static> {
    static RADIO: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
    let radio: &'static esp_radio::Controller<'static> =
        RADIO.init(esp_radio::init().expect("esp-radio init"));

    let (mut controller, interfaces) =
        esp_radio::wifi::new(radio, wifi, esp_radio::wifi::Config::default())
            .expect("wifi init");

    let client = ClientConfig::default()
        .with_ssid(alloc::string::String::from(wifi_secrets::WIFI_SSID))
        .with_password(alloc::string::String::from(wifi_secrets::WIFI_PASSWORD))
        .with_auth_method(AuthMethod::WpaWpa2Personal);
    controller
        .set_config(&ModeConfig::Client(client))
        .expect("wifi config");

    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let resources = RESOURCES.init(StackResources::new());
    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        NetConfig::dhcpv4(Default::default()),
        resources,
        0x6564_6761_7061_6431, // "edgapad1" — stack random seed
    );

    spawner.spawn(net_task(runner)).ok();
    spawner.spawn(connection_task(controller)).ok();

    stack
}
