"""Read the DW3000 DEV_ID from a DWM3000EVB shield on the NUCLEO-N657X0-Q.

No firmware: the debugger bit-bangs SPI on the Arduino header through the
N6 GPIO registers (pyOCD, core left running). The DW3000 SPI is static, so a
slow clock is fine. Pins (Arduino R3 -> N6): D10 CS=PA3, D13 SCK=PE15,
D11 MOSI=PG2, D12 MISO=PG1. Pin modes are restored afterwards.

Expected DEV_ID: 0xDECA03xx (RIDTAG 0xDECA, model 0x03).
"""
import sys

from pyocd.core.helpers import ConnectHelper

AHB4 = 0x56020000
GPIO = {"A": AHB4 + 0x0000, "D": AHB4 + 0x0C00, "E": AHB4 + 0x1000, "G": AHB4 + 0x1800}
MODER, PUPDR, IDR, BSRR = 0x00, 0x0C, 0x10, 0x18
RCC_AHB4ENR, RCC_AHB4ENSR = 0x5602825C, 0x56028A5C
PORT_EN_BIT = {"A": 0, "D": 3, "E": 4, "G": 6}

CS, SCK, MOSI, MISO = ("A", 3), ("E", 15), ("G", 2), ("G", 1)


def main():
    with ConnectHelper.session_with_chosen_probe(
        target_override="cortex_m", connect_mode="attach", options={"frequency": 4_000_000}
    ) as s:
        t = s.target
        r, w = t.read32, t.write32

        # Clock the GPIO ports we use.
        w(RCC_AHB4ENSR, sum(1 << PORT_EN_BIT[p] for p in ("A", "E", "G")))
        print(f"RCC AHB4ENR=0x{r(RCC_AHB4ENR):08x}")

        saved = {p: (r(GPIO[p] + MODER), r(GPIO[p] + PUPDR)) for p in ("A", "E", "G")}

        def mode(pin, m):          # 0 input, 1 output
            port, n = pin
            a = GPIO[port] + MODER
            w(a, (r(a) & ~(3 << 2 * n)) | (m << 2 * n))

        def put(pin, level):
            port, n = pin
            w(GPIO[port] + BSRR, 1 << (n if level else n + 16))

        def get(pin):
            port, n = pin
            return (r(GPIO[port] + IDR) >> n) & 1

        try:
            put(CS, 1); put(SCK, 0); put(MOSI, 0)
            for pin in (CS, SCK, MOSI):
                mode(pin, 1)
            mode(MISO, 0)
            pp = GPIO["G"] + PUPDR       # weak pull-up on MISO so a dead line reads 1s
            w(pp, (r(pp) & ~(3 << 2 * MISO[1])) | (1 << 2 * MISO[1]))

            def xfer(byte_out):
                byte_in = 0
                for bit in range(7, -1, -1):        # SPI mode 0, MSB first
                    put(MOSI, (byte_out >> bit) & 1)
                    put(SCK, 1)
                    byte_in = (byte_in << 1) | get(MISO)
                    put(SCK, 0)
                return byte_in

            put(CS, 0)
            xfer(0x00)                               # short read of register 0x00 (DEV_ID)
            raw = [xfer(0x00) for _ in range(4)]
            put(CS, 1)
        finally:
            for p, (m, pu) in saved.items():
                w(GPIO[p] + MODER, m)
                w(GPIO[p] + PUPDR, pu)

        dev_id = raw[0] | raw[1] << 8 | raw[2] << 16 | raw[3] << 24
        print("bytes:", " ".join(f"{b:02x}" for b in raw))
        print(f"DEV_ID = 0x{dev_id:08X}")
        if dev_id >> 16 == 0xDECA and (dev_id >> 8) & 0xFF == 0x03:
            print(f"PASS  DW3000 found: RIDTAG 0xDECA, model 0x03, ver {dev_id >> 4 & 0xF}, rev {dev_id & 0xF}")
            return 0
        print("FAIL  no DW3000 answer (all 0xFF = MISO floating/pulled up; all 0x00 = MISO stuck low)")
        return 1


if __name__ == "__main__":
    sys.exit(main())
