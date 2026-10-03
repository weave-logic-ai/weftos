//! WeftOS node firmware for the NUCLEO-N657X0-Q.
//!
//! - Clocks: HSE 48 MHz crystal (measured −2 ppm) → PLL1 800 MHz →
//!   CPU 400 MHz (IC1 /2), system buses 200 MHz (IC2/IC6/IC11 /4). The
//!   internal HSI is 0.5 % off, so nothing timing-related runs from it.
//! - Ethernet: ETH1 over RMII to the Nucleo's PHY (pins from ST's
//!   Nx_PTP_Client: REF_CLK PF7, CRS_DV PF10, RXD0/1 PF14/PF15,
//!   TX_EN PF11, TXD0/1 PF12/PF13, MDIO PF4, MDC PG11).
//! - Network: DHCPv4, falling back to static 192.168.10.2/24 after 5 s (for a
//!   direct cable to a host).
//! - PTP: the ETH1 MAC's IEEE 1588 clock, disciplined by a PTPv2 slave
//!   against a master on the link (see `ptp`).
//! - UWB: reads the DWM3000EVB's DEV_ID over SPI5 on the Arduino header
//!   (SCK PE15, MOSI PG2, MISO PG1, CS PA3), expecting 0xDECA03xx.
//! - Announce: every 2 s, a `weftos-leaf-types::LeafServices` CBOR message
//!   is broadcast on UDP port 47006 so hosts on the LAN can find the node.
#![no_std]
#![no_main]

extern crate alloc;

mod ptp;

use alloc::string::String;
use core::ptr::addr_of_mut;

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_net::udp::UdpSocket;
use embassy_net::wire::{IpCidr, Ipv4Addr, Ipv4Cidr};
use embassy_net::{Stack, StackStorage};
use embassy_stm32::eth::{Ethernet, GenericPhy, PacketQueue, Sma};
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::spi::{self, Spi};
use embassy_stm32::peripherals::{ETH1, ETH_SMA};
use embassy_stm32::rcc::{
    CpuClk, Hse, HseMode, IcConfig, Icint, Icsel, Pll, Plldivm, Pllpdiv, Pllsel, SupplyConfig, SysClk,
};
use embassy_stm32::time::Hertz;
use embassy_stm32::{bind_interrupts, eth, Config};
use embassy_time::{with_timeout, Duration, Timer};
use embedded_alloc::LlffHeap as Heap;
use panic_probe as _;
use static_cell::StaticCell;
use weftos_leaf_types::{encode, ComputeCap, LeafServices};

bind_interrupts!(struct Irqs {
    ETH1 => eth::InterruptHandler<ETH1>;
});

type Device = Ethernet<'static, ETH1, GenericPhy<Sma<'static, ETH_SMA>>>;

#[global_allocator]
static HEAP: Heap = Heap::empty();

const ANNOUNCE_PORT: u16 = 47006;
/// Locally administered unicast MAC ("WFTN6").
const MAC: [u8; 6] = [0x02, 0x57, 0x46, 0x54, 0x4E, 0x36];

fn rcc_config() -> Config {
    let mut config = Config::default();
    // The Nucleo is powered like the DK: ST's own Nucleo projects call
    // HAL_PWREx_ConfigSupply(PWR_EXTERNAL_SOURCE_SUPPLY).
    config.rcc.supply_config = SupplyConfig::External;
    config.rcc.hse = Some(Hse { freq: Hertz(48_000_000), mode: HseMode::Oscillator });
    // PLL1 = HSE 48 MHz / 3 * 50 = 800 MHz (ST's Nucleo PLL1 settings).
    config.rcc.pll1 = Some(Pll::Oscillator {
        source: Pllsel::Hse,
        divm: Plldivm::Div3,
        fractional: 0,
        divn: 50,
        divp1: Pllpdiv::Div1,
        divp2: Pllpdiv::Div1,
    });
    config.rcc.ic1 = Some(IcConfig { source: Icsel::Pll1, divider: Icint::Div2 }); // CPU 400 MHz
    let bus = IcConfig { source: Icsel::Pll1, divider: Icint::Div4 }; // 200 MHz
    config.rcc.ic2 = Some(bus);
    config.rcc.ic6 = Some(bus);
    config.rcc.ic11 = Some(bus);
    config.rcc.cpu = CpuClk::Ic1;
    config.rcc.sys = SysClk::Ic2;
    config
}

/// Read the DW3000 DEV_ID register (short-addressed read of register 0x00).
fn read_dw3000_id(spi: &mut Spi<'_, embassy_stm32::mode::Blocking, spi::mode::Master>, cs: &mut Output<'_>) -> u32 {
    let mut buf = [0x00u8, 0, 0, 0, 0];
    cs.set_low();
    let ok = spi.blocking_transfer_in_place(&mut buf).is_ok();
    cs.set_high();
    if !ok {
        return 0;
    }
    u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]])
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn ptp_task(stack: Stack<'static>, clock_id: [u8; 8], nominal_addend: u32) -> ! {
    ptp::run(stack, clock_id, nominal_addend).await
}

fn announce() -> LeafServices {
    LeafServices {
        node_pubkey: [0x6e; 32],
        hostname: String::from("n6-node"),
        firmware_version: String::from(env!("CARGO_PKG_VERSION")),
        audio_sink: None,
        display_sink: None,
        compute: Some(ComputeCap { cpu_mhz: 400, free_heap_bytes: HEAP.free() as u32, eml_core: false }),
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    {
        const HEAP_SIZE: usize = 16 * 1024;
        static mut HEAP_MEM: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
        unsafe { HEAP.init(addr_of_mut!(HEAP_MEM) as usize, HEAP_SIZE) }
    }

    let p = embassy_stm32::init(rcc_config());
    info!("weftos-n6-node: clocks up (HSE 48 MHz -> PLL1 800 MHz, CPU 400 MHz)");

    // UWB: DWM3000EVB on the Arduino header, SPI5 mode 0 at ~2 MHz.
    let mut spi_cfg = spi::Config::default();
    spi_cfg.frequency = Hertz(2_000_000);
    let mut uwb_spi = Spi::new_blocking(p.SPI5, p.PE15, p.PG2, p.PG1, spi_cfg);
    let mut uwb_cs = Output::new(p.PA3, Level::High, Speed::VeryHigh);
    let dev_id = read_dw3000_id(&mut uwb_spi, &mut uwb_cs);
    if dev_id >> 16 == 0xDECA {
        info!("UWB: DW3000 DEV_ID 0x{:08x} (model 0x{:02x}, rev {})", dev_id, (dev_id >> 8) & 0xFF, dev_id & 0xF);
    } else {
        warn!("UWB: no DW3000 answer (DEV_ID 0x{:08x})", dev_id);
    }

    static PACKETS: StaticCell<PacketQueue<4, 4>> = StaticCell::new();
    let device = Ethernet::new(
        PACKETS.init(PacketQueue::<4, 4>::new()),
        p.ETH1,
        p.PF7,  // REF_CLK
        p.PF10, // CRS_DV
        p.PF14, // RXD0
        p.PF15, // RXD1
        p.PF12, // TXD0
        p.PF13, // TXD1
        p.PF11, // TX_EN
        MAC,
        p.ETH_SMA,
        p.PF4,  // MDIO
        p.PG11, // MDC
        Irqs,
    );

    let hclk = embassy_stm32::rcc::frequency::<ETH1>().0;
    let nominal_addend = match ptp::start(hclk) {
        Some(st) => {
            info!("PTP clock started: bus {} Hz, step {} ns, addend 0x{:08x}", st.hclk, st.ssinc_ns, st.addend);
            Some(st.addend)
        }
        None => {
            warn!("PTP clock did not start (MAC did not acknowledge)");
            None
        }
    };

    static STACK: StaticCell<StackStorage> = StaticCell::new();
    let (stack, runner) = embassy_net::Stack::new(STACK.init(StackStorage::new()), 0x5746_544e_3600_0001);
    static DEVICE: StaticCell<Device> = StaticCell::new();
    let iface = unwrap!(stack.add_iface_borrowed(DEVICE.init(device)));
    unwrap!(iface.set_dhcpv4(Some(Default::default())));
    spawner.spawn(unwrap!(net_task(runner)));

    info!("waiting for Ethernet link");
    iface.wait_link_up().await;
    info!("link up; waiting for DHCP");
    if with_timeout(Duration::from_secs(5), iface.wait_config_up()).await.is_ok() {
        info!("DHCP configured");
    } else {
        unwrap!(iface.set_dhcpv4(None));
        unwrap!(iface.add_ip_addr(IpCidr::V4(Ipv4Cidr::new(Ipv4Addr::new(192, 168, 10, 2), 24))));
        info!("no DHCP server; static 192.168.10.2/24");
    }

    unwrap!(iface.join_multicast_group(ptp::MCAST));
    if let Some(addend) = nominal_addend {
        // EUI-64 clock identity from the MAC: 02:57:46:ff:fe:54:4e:36.
        let clock_id = [MAC[0], MAC[1], MAC[2], 0xFF, 0xFE, MAC[3], MAC[4], MAC[5]];
        spawner.spawn(unwrap!(ptp_task(stack, clock_id, addend)));
    }

    // embassy-net (xarxa stack): sockets own their buffers; bind local port, any remote.
    let mut sock = unwrap!(UdpSocket::new(stack));
    unwrap!(sock.bind(ANNOUNCE_PORT, 0u16));

    let mut n: u32 = 0;
    loop {
        match encode(&announce()) {
            Ok(bytes) => match sock.send_to(&bytes, (core::net::Ipv4Addr::BROADCAST, ANNOUNCE_PORT)).await {
                Ok(()) => {
                    n += 1;
                    let (sec, ns) = ptp::now();
                    info!(
                        "announce #{} sent ({} bytes CBOR) to 255.255.255.255:{}; PTP time {}.{:09} s",
                        n, bytes.len(), ANNOUNCE_PORT, sec, ns
                    );
                }
                Err(e) => warn!("announce send failed: {}", e),
            },
            Err(_) => warn!("announce encode failed"),
        }
        Timer::after_secs(2).await;
    }
}
