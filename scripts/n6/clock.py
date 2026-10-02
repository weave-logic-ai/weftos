"""Measure the STM32N657 CPU clock against host time, on HSI and then on HSE.

Runs inside the n6 probe container while a program keeps the core running
(DWT CYCCNT only counts while the core runs). Reads go over SWD through pyOCD
without halting the core, so the count is never paused by the measurement.

Usage: python clock.py [seconds-per-source]
"""
import sys
import time

from pyocd.core.helpers import ConnectHelper

RCC = 0x56028000
RCC_CR, RCC_SR, RCC_CFGR1 = RCC + 0x000, RCC + 0x004, RCC + 0x020
RCC_CSR, RCC_CCR = RCC + 0x800, RCC + 0x1000          # set / clear aliases of CR
HSEON = 1 << 4
CPUSW_SHIFT, CPUSWS_SHIFT = 16, 20
SOURCES = {0: "HSI", 1: "MSI", 2: "HSE", 3: "IC1 (PLL)"}
NOMINAL = {"HSI": 64_000_000, "HSE": 48_000_000}

DEMCR, DWT_CTRL, DWT_CYCCNT = 0xE000EDFC, 0xE0001000, 0xE0001004


def wait_for(t, addr, mask, want, timeout=2.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if t.read32(addr) & mask == want:
            return True
        time.sleep(0.01)
    return False


def measure(t, seconds, period=0.05):
    """Least-squares fit of unwrapped CYCCNT against host monotonic time."""
    t.write32(DWT_CYCCNT, 0)
    xs, ys, total, last = [], [], 0, t.read32(DWT_CYCCNT)
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        before = time.monotonic_ns()
        c = t.read32(DWT_CYCCNT)
        after = time.monotonic_ns()
        total += (c - last) & 0xFFFFFFFF
        last = c
        xs.append((before + after) / 2e9)
        ys.append(total)
        time.sleep(period)
    n = len(xs)
    mx, my = sum(xs) / n, sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs)
    slope = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sxx
    resid = [y - (my + slope * (x - mx)) for x, y in zip(xs, ys)]
    se = (sum(r * r for r in resid) / (n - 2) / sxx) ** 0.5
    jitter_us = max(abs(r) for r in resid) / slope * 1e6
    return slope, se, n, jitter_us


def report(name, hz, se, n, jitter_us, seconds):
    nominal = NOMINAL.get(name)
    line = f"{name:<4} {hz / 1e6:12.6f} MHz  ±{se / hz * 1e6:5.2f} ppm (1σ, {n} samples, {seconds:.0f} s, worst sample jitter {jitter_us:.0f} µs)"
    if nominal:
        line += f"  -> {(hz - nominal) / nominal * 1e6:+9.1f} ppm vs nominal {nominal / 1e6:g} MHz"
    print(line, flush=True)


def main():
    seconds = float(sys.argv[1]) if len(sys.argv) > 1 else 60
    with ConnectHelper.session_with_chosen_probe(
        target_override="cortex_m", connect_mode="attach", options={"frequency": 4_000_000}
    ) as s:
        t = s.target
        if t.is_halted():   # CYCCNT does not count while halted
            print("core was halted; resuming it", flush=True)
            t.resume()
        t.write32(DEMCR, t.read32(DEMCR) | (1 << 24))       # TRCENA
        t.write32(DWT_CTRL, t.read32(DWT_CTRL) | 1)         # CYCCNTENA

        cfgr1 = t.read32(RCC_CFGR1)
        start_src = SOURCES[(cfgr1 >> CPUSWS_SHIFT) & 3]
        print(f"RCC CR=0x{t.read32(RCC_CR):08x} SR=0x{t.read32(RCC_SR):08x} "
              f"CFGR1=0x{cfgr1:08x}; CPU clock source: {start_src}", flush=True)
        report(start_src, *measure(t, seconds), seconds)

        # Turn on the HSE (48 MHz on the Nucleo) and move the CPU onto it.
        hse_was_on = t.read32(RCC_CR) & HSEON
        t.write32(RCC_CSR, HSEON)
        if not wait_for(t, RCC_SR, HSEON, HSEON):
            print("HSE did not become ready within 2 s (no crystal, or it needs bypass mode)")
            t.write32(RCC_CCR, HSEON)
            return 1
        t.write32(RCC_CFGR1, (cfgr1 & ~(3 << CPUSW_SHIFT)) | (2 << CPUSW_SHIFT))
        try:
            if not wait_for(t, RCC_CFGR1, 3 << CPUSWS_SHIFT, 2 << CPUSWS_SHIFT):
                print("CPU did not switch to HSE")
                return 1
            print("CPU clock source: HSE", flush=True)
            report("HSE", *measure(t, seconds), seconds)
        finally:
            # Put everything back the way we found it.
            t.write32(RCC_CFGR1, cfgr1)
            wait_for(t, RCC_CFGR1, 3 << CPUSWS_SHIFT, cfgr1 & (3 << CPUSWS_SHIFT))
            if not hse_was_on:
                t.write32(RCC_CCR, HSEON)
            print(f"restored CFGR1=0x{t.read32(RCC_CFGR1):08x} CR=0x{t.read32(RCC_CR):08x}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
