//! WeftOS node firmware for the NUCLEO-N657X0-Q.
//!
//! - Clocks: HSE 48 MHz crystal (measured −2 ppm) → PLL1 800 MHz →
//!   CPU 400 MHz (IC1 /2), system buses 200 MHz (IC2/IC6/IC11 /4). The
//!   internal HSI is 0.5 % off, so nothing timing-related runs from it.
//! - Ethernet: ETH1 over RMII to the Nucleo's PHY (pins from ST's
//!   Nx_PTP_Client: REF_CLK PF7, CRS_DV PF10, RXD0/1 PF14/PF15,
//!   TX_EN PF11, TXD0/1 PF12/PF13, MDIO PF4, MDC PG11), DHCPv4.
//! - Announce: every 2 s, a `weftos-leaf-types::LeafServices` CBOR message
//!   is broadcast on UDP port 47006 so hosts on the LAN can find the node.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use core::ptr::addr_of_mut;

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_net::udp::UdpSocket;
use embassy_net::StackStorage;
use embassy_stm32::eth::{Ethernet, GenericPhy, PacketQueue, Sma};
use embassy_stm32::peripherals::{ETH1, ETH_SMA};
use embassy_stm32::rcc::{
    CpuClk, Hse, HseMode, IcConfig, Icint, Icsel, Pll, Plldivm, Pllpdiv, Pllsel, SupplyConfig, SysClk,
};
use embassy_stm32::time::Hertz;
use embassy_stm32::{bind_interrupts, eth, Config};
use embassy_time::Timer;
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

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static>) -> ! {
    runner.run().await
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

    static STACK: StaticCell<StackStorage> = StaticCell::new();
    let (stack, runner) = embassy_net::Stack::new(STACK.init(StackStorage::new()), 0x5746_544e_3600_0001);
    static DEVICE: StaticCell<Device> = StaticCell::new();
    let iface = unwrap!(stack.add_iface_borrowed(DEVICE.init(device)));
    unwrap!(iface.set_dhcpv4(Some(Default::default())));
    spawner.spawn(unwrap!(net_task(runner)));

    info!("waiting for Ethernet link");
    iface.wait_link_up().await;
    info!("link up; waiting for DHCP");
    iface.wait_config_up().await;
    info!("DHCP configured");

    // embassy-net (xarxa stack): sockets own their buffers; bind local port, any remote.
    let mut sock = unwrap!(UdpSocket::new(stack));
    unwrap!(sock.bind(ANNOUNCE_PORT, 0u16));

    let mut n: u32 = 0;
    loop {
        match encode(&announce()) {
            Ok(bytes) => match sock.send_to(&bytes, (core::net::Ipv4Addr::BROADCAST, ANNOUNCE_PORT)).await {
                Ok(()) => {
                    n += 1;
                    info!("announce #{} sent ({} bytes CBOR) to 255.255.255.255:{}", n, bytes.len(), ANNOUNCE_PORT);
                }
                Err(e) => warn!("announce send failed: {}", e),
            },
            Err(_) => warn!("announce encode failed"),
        }
        Timer::after_secs(2).await;
    }
}
