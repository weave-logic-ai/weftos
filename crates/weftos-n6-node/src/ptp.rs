//! IEEE 1588 on the ETH1 MAC: the hardware clock, and a minimal PTPv2 slave.
//!
//! **Clock.** embassy-stm32's `ptp` feature targets the H5/H7 MAC and does not
//! build for the N6 (its MACSSIR has no SNSINC field), so this drives the
//! timestamp registers directly: nanosecond rollover, fine update. In fine mode
//! the accumulator adds `addend` every bus clock and steps the time by
//! `ssinc_ns` on each overflow, so `addend = 2^32 * (1e9 / ssinc_ns) / hclk`.
//! The servo steers the rate by scaling the addend.
//!
//! **Protocol.** End-to-end delay, two-step, UDP/IPv4 multicast
//! (224.0.1.129, event port 319, general port 320), domain 0, slave only:
//! Sync (t2 local receive) + Follow_Up (t1) → Delay_Req (t3 local send) →
//! Delay_Resp (t4). offset = ((t2 − t1) − (t4 − t3)) / 2. The first offset
//! steps the clock; after that a PI servo adjusts frequency.
//!
//! **Limit.** t2 and t3 are read from the hardware clock in software, when the
//! packet reaches or leaves this task, not by the MAC on the wire. Per-packet
//! hardware timestamps need the descriptor timestamping that sits behind
//! embassy's `ptp` feature.
use core::net::Ipv4Addr;
use core::ptr::{read_volatile, write_volatile};

use defmt::*;
use embassy_futures::select::{select, Either};
use embassy_net::udp::UdpSocket;
use embassy_net::Stack;

const MAC: usize = 0x5803_6000; // ETH1 (secure alias)
const MACTSCR: usize = MAC + 0xB00;
const MACSSIR: usize = MAC + 0xB04;
const MACSTSR: usize = MAC + 0xB08;
const MACSTNR: usize = MAC + 0xB0C;
const MACSTSUR: usize = MAC + 0xB10;
const MACSTNUR: usize = MAC + 0xB14;
const MACTSAR: usize = MAC + 0xB18;

const TSENA: u32 = 1 << 0;
const TSCFUPDT: u32 = 1 << 1;
const TSINIT: u32 = 1 << 2;
const TSADDREG: u32 = 1 << 5;
const TSCTRLSSR: u32 = 1 << 9; // subseconds count nanoseconds, roll over at 1e9

pub const MCAST: Ipv4Addr = Ipv4Addr::new(224, 0, 1, 129);
const EVENT_PORT: u16 = 319;
const GENERAL_PORT: u16 = 320;

const MSG_SYNC: u8 = 0x0;
const MSG_FOLLOW_UP: u8 = 0x8;
const MSG_DELAY_RESP: u8 = 0x9;
const MSG_ANNOUNCE: u8 = 0xB;

/// Once locked, offsets larger than this are stepped instead of slewed.
const STEP_THRESHOLD_NS: i64 = 1_000_000;
/// PI gains per exchange (1 s). Gentle, because software timestamps on both
/// ends make individual offsets noisy.
const KP: f64 = 0.3;
const KI: f64 = 0.05;
const MAX_PPB: f64 = 500_000.0;
/// Lucky-packet filter: use an exchange only if its path delay is within this
/// much of the smallest delay seen in the last `DELAY_WINDOW` exchanges.
/// Queueing only ever adds delay, so the fastest exchanges are the cleanest.
const DELAY_SLACK_NS: i64 = 30_000;
const DELAY_WINDOW: usize = 16;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn wait_clear(a: usize, bit: u32) -> bool {
    for _ in 0..1_000_000 {
        if rd(a) & bit == 0 {
            return true;
        }
    }
    false
}

pub struct Started {
    pub hclk: u32,
    pub ssinc_ns: u32,
    pub addend: u32,
}

/// Start the clock at 0 s. Returns the settings, or None if the MAC never acknowledged.
pub fn start(hclk: u32) -> Option<Started> {
    // The step must be slower than the bus clock: ~2 bus cycles per step.
    let ssinc_ns = (2_000_000_000u64).div_ceil(u64::from(hclk)).max(1) as u32;
    let addend = (((1u128 << 32) * 1_000_000_000) / (u128::from(ssinc_ns) * u128::from(hclk))) as u32;
    wr(MACTSCR, TSENA | TSCTRLSSR);
    wr(MACSSIR, (ssinc_ns & 0xFF) << 16);
    if !set_addend(addend) {
        return None;
    }
    wr(MACTSCR, rd(MACTSCR) | TSCFUPDT);
    if !set_time(0) {
        return None;
    }
    Some(Started { hclk, ssinc_ns, addend })
}

fn set_addend(addend: u32) -> bool {
    wr(MACTSAR, addend);
    wr(MACTSCR, rd(MACTSCR) | TSADDREG);
    wait_clear(MACTSCR, TSADDREG)
}

/// Set the clock to `ns` nanoseconds since the PTP epoch.
fn set_time(ns: i64) -> bool {
    let ns = ns.max(0) as u64;
    wr(MACSTSUR, (ns / 1_000_000_000) as u32);
    wr(MACSTNUR, (ns % 1_000_000_000) as u32);
    wr(MACTSCR, rd(MACTSCR) | TSINIT);
    wait_clear(MACTSCR, TSINIT)
}

/// Current (seconds, nanoseconds), re-reading across a seconds rollover.
pub fn now() -> (u32, u32) {
    loop {
        let s = rd(MACSTSR);
        let n = rd(MACSTNR) & 0x7FFF_FFFF;
        if rd(MACSTSR) == s {
            return (s, n);
        }
    }
}

fn now_ns() -> i64 {
    let (s, n) = now();
    i64::from(s) * 1_000_000_000 + i64::from(n)
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

/// A PTP timestamp (48-bit seconds, 32-bit nanoseconds) at `at`, in ns.
fn timestamp_ns(b: &[u8], at: usize) -> i64 {
    let mut s = [0u8; 8];
    s[2..8].copy_from_slice(&b[at..at + 6]);
    let secs = u64::from_be_bytes(s) as i64;
    let nanos = u32::from_be_bytes(b[at + 6..at + 10].try_into().unwrap()) as i64;
    secs * 1_000_000_000 + nanos
}

/// correctionField, scaled nanoseconds (ns << 16).
fn correction_ns(b: &[u8]) -> i64 {
    i64::from_be_bytes(b[8..16].try_into().unwrap()) >> 16
}

fn abs(x: i64) -> i64 {
    if x < 0 {
        -x
    } else {
        x
    }
}

struct Servo {
    nominal_addend: u32,
    integral: f64,
    stepped: bool,
    delays: [i64; DELAY_WINDOW],
    next: usize,
    seen: usize,
}

enum Outcome {
    Stepped,
    Filtered { min_delay: i64 },
    Slewed { ppb: f64 },
}

impl Servo {
    fn new(nominal_addend: u32) -> Self {
        Self { nominal_addend, integral: 0.0, stepped: false, delays: [i64::MAX; DELAY_WINDOW], next: 0, seen: 0 }
    }

    /// Feed one exchange (offset = local − master, path delay; ns).
    fn update(&mut self, offset: i64, delay: i64) -> Outcome {
        self.delays[self.next] = delay;
        self.next = (self.next + 1) % DELAY_WINDOW;
        self.seen += 1;
        let min_delay = self.delays.iter().copied().min().unwrap_or(delay);
        if self.stepped && self.seen >= 4 && delay > min_delay + DELAY_SLACK_NS {
            return Outcome::Filtered { min_delay };
        }
        match self.slew(offset) {
            None => Outcome::Stepped,
            Some(ppb) => Outcome::Slewed { ppb },
        }
    }

    fn slew(&mut self, offset: i64) -> Option<f64> {
        if !self.stepped || abs(offset) > STEP_THRESHOLD_NS {
            set_time(now_ns() - offset);
            self.stepped = true;
            self.integral = 0.0;
            return None;
        }
        let off = offset as f64;
        self.integral += off;
        let mut ppb = -(KP * off + KI * self.integral);
        if ppb > MAX_PPB {
            ppb = MAX_PPB;
        } else if ppb < -MAX_PPB {
            ppb = -MAX_PPB;
        }
        let addend = f64::from(self.nominal_addend) * (1.0 + ppb * 1e-9);
        set_addend(addend as u32);
        Some(ppb)
    }
}

fn delay_req(clock_id: &[u8; 8], seq: u16) -> [u8; 44] {
    let mut m = [0u8; 44];
    m[0] = 0x01; // transportSpecific 0, messageType Delay_Req
    m[1] = 0x02; // versionPTP 2
    m[2..4].copy_from_slice(&44u16.to_be_bytes());
    m[20..28].copy_from_slice(clock_id);
    m[28..30].copy_from_slice(&1u16.to_be_bytes()); // portNumber
    m[30..32].copy_from_slice(&seq.to_be_bytes());
    m[32] = 0x01; // controlField: Delay_Req
    m[33] = 0x7F; // logMessageInterval
    m
}

/// Run the slave forever on `stack`.
pub async fn run(stack: Stack<'static>, clock_id: [u8; 8], nominal_addend: u32) -> ! {
    let mut ev = unwrap!(UdpSocket::new(stack));
    unwrap!(ev.bind(EVENT_PORT, 0u16));
    let mut gen = unwrap!(UdpSocket::new(stack));
    unwrap!(gen.bind(GENERAL_PORT, 0u16));

    let mut servo = Servo::new(nominal_addend);
    let (mut sync_seq, mut t2, mut sync_corr) = (None::<u16>, 0i64, 0i64);
    let (mut t1, mut req_seq, mut t3, mut req_pending) = (0i64, 0u16, 0i64, false);
    let mut master_logged = false;
    let mut ebuf = [0u8; 128];
    let mut gbuf = [0u8; 128];

    loop {
        let got = select(
            ev.recv_from_with(|d, _| {
                let t = now_ns();
                let n = d.len().min(128);
                ebuf[..n].copy_from_slice(&d[..n]);
                (n, t)
            }),
            gen.recv_from_with(|d, _| {
                let n = d.len().min(128);
                gbuf[..n].copy_from_slice(&d[..n]);
                n
            }),
        )
        .await;

        match got {
            Either::First(Ok((n, rx_t))) if n >= 44 && ebuf[0] & 0x0F == MSG_SYNC => {
                sync_seq = Some(be16(&ebuf, 30));
                t2 = rx_t;
                sync_corr = correction_ns(&ebuf);
                if ebuf[6] & 0x02 == 0 {
                    // One-step master: the Sync carries t1 itself.
                    t1 = timestamp_ns(&ebuf, 34) + sync_corr;
                    sync_seq = None;
                    req_seq = req_seq.wrapping_add(1);
                    t3 = now_ns();
                    req_pending = ev.send_to(&delay_req(&clock_id, req_seq), (MCAST, EVENT_PORT)).await.is_ok();
                }
            }
            Either::Second(Ok(n)) if n >= 44 => match gbuf[0] & 0x0F {
                MSG_FOLLOW_UP if sync_seq == Some(be16(&gbuf, 30)) => {
                    t1 = timestamp_ns(&gbuf, 34) + sync_corr + correction_ns(&gbuf);
                    sync_seq = None;
                    req_seq = req_seq.wrapping_add(1);
                    t3 = now_ns();
                    req_pending = ev.send_to(&delay_req(&clock_id, req_seq), (MCAST, EVENT_PORT)).await.is_ok();
                }
                MSG_DELAY_RESP
                    if n >= 54 && req_pending && be16(&gbuf, 30) == req_seq && gbuf[44..52] == clock_id[..] =>
                {
                    req_pending = false;
                    let t4 = timestamp_ns(&gbuf, 34) - correction_ns(&gbuf);
                    let ms = t2 - t1;
                    let sm = t4 - t3;
                    let offset = (ms - sm) / 2;
                    let delay = (ms + sm) / 2;
                    match servo.update(offset, delay) {
                        Outcome::Stepped => info!("ptp: stepped clock by {} ns (path delay {} ns)", -offset, delay),
                        Outcome::Filtered { min_delay } => {
                            info!("ptp: skipped (path delay {} ns > min {} ns + slack)", delay, min_delay)
                        }
                        Outcome::Slewed { ppb } => {
                            info!("ptp: offset {} ns, path delay {} ns, freq {} ppb", offset, delay, ppb as i32)
                        }
                    }
                }
                MSG_ANNOUNCE if !master_logged => {
                    master_logged = true;
                    info!(
                        "ptp: master {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        gbuf[20], gbuf[21], gbuf[22], gbuf[23], gbuf[24], gbuf[25], gbuf[26], gbuf[27]
                    );
                }
                _ => {}
            },
            _ => {}
        }
    }
}
