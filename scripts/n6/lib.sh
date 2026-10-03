# Sourced inside the probe container by scripts/n6.sh. Expects $CHIP.

rd() { probe-rs read --chip "$CHIP" b32 "$1" 1 | awk '{print $2}'; }

# Address of the first ALLOC+LOAD section, e.g. 0x34180400.
vector_base() {
  arm-none-eabi-objdump -h "$1" | awk '
    /^ *[0-9]+ / { addr = $4; getline; if ($0 ~ /ALLOC/ && $0 ~ /LOAD/) { print "0x" addr; exit } }'
}

# Load a RAM-linked ELF and start it without resetting into it.
# `probe-rs run` resets and halts at the reset vector, before the boot ROM has
# enabled AXISRAM, so its RAM writes fail (SwdApWdataError) and the core locks
# up. Instead: plain reset (the ROM brings up RAM and waits in dev boot), then
# GDB loads the image, points SP/PC at it and detaches, which resumes the core.
load_and_start() {
  local elf="$1" entry
  # The vector table is the first loaded section (not the LOAD segment start,
  # which is page-aligned). Word 0 is the initial SP.
  VECTORS=$(vector_base "$elf")
  entry=$(printf '0x%08x' $(( $(arm-none-eabi-readelf -hW "$elf" | awk '/Entry point/ { print $4 }') & ~1 )))
  # The reset logs SwdApWdataError warnings while the ROM gates RAM; harmless,
  # and the load is verified afterwards.
  probe-rs reset --chip "$CHIP" >/dev/null 2>&1
  sleep 1
  # Clock AXISRAM1 and AXISRAM2 (RCC.MEMENSR bits 7, 8): images such as the
  # NPU example put loadable sections in AXISRAM1, which is off at reset.
  probe-rs write --chip "$CHIP" b32 0x56028A4C 0x180 >/dev/null 2>&1 || true
  probe-rs gdb --chip "$CHIP" --gdb-connection-string 127.0.0.1:1337 >/tmp/gdbsrv.log 2>&1 &
  local srv=$! _
  for _ in $(seq 1 30); do grep -q "GDB stub" /tmp/gdbsrv.log 2>/dev/null && break; sleep 0.2; done
  # shellcheck disable=SC2016  # $pc, $sp, $xpsr are GDB variables
  gdb-multiarch -q -batch -nx "$elf" \
    -ex 'target remote 127.0.0.1:1337' \
    -ex 'monitor halt' \
    -ex 'load' \
    -ex "set \$sp = *(unsigned int*)$VECTORS" \
    -ex "set \$pc = $entry" \
    -ex 'printf "start pc=0x%08x sp=0x%08x xpsr=0x%08x\n", $pc, $sp, $xpsr' \
    -ex 'detach' 2>&1 | grep -E "^(start|Loading|Start address)" || true
  sleep 1   # let the server finish resuming the core before it is stopped
  kill "$srv" 2>/dev/null || true
  wait "$srv" 2>/dev/null || true
  ensure_running
}

# The GDB detach sometimes leaves the core halted. pyOCD attaches without
# halting, so it can check DHCSR.S_HALT and resume without disturbing a running core.
ensure_running() {
  python3 -c '
from pyocd.core.helpers import ConnectHelper
with ConnectHelper.session_with_chosen_probe(target_override="cortex_m", connect_mode="attach") as s:
    if s.target.is_halted():
        s.target.resume()
        print("ensure_running: core was halted after load; resumed")
' 2> >(grep -v -i "disk devices by id" >&2)
}
