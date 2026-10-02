#!/usr/bin/env bash
# NUCLEO-N657X0-Q (STM32N657, Cortex-M55) harness.
#
# macOS USB stalls the board's STLINK-V3EC (libusb "pipe is stalled"), so the
# probe is passed through OrbStack and probe-rs runs in a linux/arm64 container
# built from scripts/n6/Dockerfile.
#
#   scripts/n6.sh attach        pass the ST-LINK through to OrbStack
#   scripts/n6.sh detach        give it back to macOS
#   scripts/n6.sh probe         list probes and read the core CPUID
#   scripts/n6.sh smoke         end-to-end check (probe, CPUID, RAM, code runs)
#   scripts/n6.sh clock [secs]  measure the CPU clock on HSI and HSE against host time
#   scripts/n6.sh run <elf>     load a RAM-linked ELF into AXISRAM and start it
#   scripts/n6.sh shell         interactive shell with probe-rs available
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
N6_DIR="$REPO/scripts/n6"
# Tag by Dockerfile content so edits rebuild the image.
IMAGE="weftos-n6-probe:$(shasum -a 256 "$N6_DIR/Dockerfile" | cut -c1-12)"
CHIP="STM32N657"
STLINK_VIDPID="0483:3754"

die() { echo "n6: $*" >&2; exit 1; }

stlink_id() { orb usb list 2>/dev/null | awk -v v="$STLINK_VIDPID" '$2 == v { print $1; exit }'; }

cmd_attach() {
  local id; id="$(stlink_id)"
  [[ -n "$id" ]] || die "no ST-LINK ($STLINK_VIDPID) visible to OrbStack; check the USB cable"
  if orb usb list | awk -v i="$id" '$1 == i' | grep -q attached; then
    echo "n6: ST-LINK $id already attached"
  else
    orb usb attach "$id"
  fi
}

cmd_detach() {
  local id; id="$(stlink_id)"
  [[ -n "$id" ]] || die "no ST-LINK visible"
  orb usb detach "$id"
}

ensure_image() {
  docker image inspect "$IMAGE" >/dev/null 2>&1 \
    || docker build -q -t "$IMAGE" "$N6_DIR" >/dev/null
}

# Run a script inside the probe container with the USB bus and repo files mounted.
in_container() {
  ensure_image
  cmd_attach >/dev/null
  docker run --rm -i --privileged \
    -v /dev/bus/usb:/dev/bus/usb \
    -v "$N6_DIR:/work/n6:ro" \
    ${N6_EXTRA_MOUNT:+-v "$N6_EXTRA_MOUNT"} \
    -e CHIP="$CHIP" -e N6_SECS="${N6_SECS:-}" \
    "$IMAGE" bash -euo pipefail -s
}

cmd_probe() {
  in_container <<'EOF'
probe-rs list
echo "CPUID: $(probe-rs read --chip "$CHIP" b32 0xE000ED00 1 | awk '{print $2}')"
EOF
}

cmd_smoke() {
  in_container <<'EOF'
. /work/n6/lib.sh
pass() { echo "PASS  $*"; }
fail() { echo "FAIL  $*"; exit 1; }

# 1. The probe opens.
probe-rs list | grep -q "STLink V3" && pass "ST-LINK V3 opens" || fail "no ST-LINK"

# 2. The core answers: CPUID part number 0xD22 is Cortex-M55.
cpuid=$(rd 0xE000ED00)
[[ $(( (0x$cpuid >> 4) & 0xFFF )) -eq $((0xD22)) ]] \
  && pass "CPUID $cpuid (Cortex-M55)" || fail "CPUID $cpuid is not a Cortex-M55"

# 3. AXISRAM write, read back, restore.
scratch=0x341C0000
orig=$(rd $scratch)
probe-rs write --chip "$CHIP" b32 $scratch 0xA5C3F00D
got=$(rd $scratch)
probe-rs write --chip "$CHIP" b32 $scratch 0x$orig
[[ $got == a5c3f00d ]] && pass "AXISRAM write/readback at $scratch (restored 0x$orig)" \
                       || fail "AXISRAM readback $got"

# 4. Code runs: assemble, load into AXISRAM, start it, watch the counter advance.
cd /tmp
arm-none-eabi-as -march=armv8.1-m.main -mthumb -o smoke.o /work/n6/smoke/smoke.S
arm-none-eabi-ld -T /work/n6/smoke/smoke.ld -o smoke.elf smoke.o
counter=0x$(arm-none-eabi-nm smoke.elf | awk '$3 == "counter" {print $1}')
load_and_start smoke.elf >/tmp/load.log   # not piped: VECTORS must reach this shell
sed 's/^/      /' /tmp/load.log
want=$(printf '%08x' "$(arm-none-eabi-nm smoke.elf | awk '$3 == "_estack" { print "0x" $1 }')")
[[ $(rd "$VECTORS") == "$want" ]] && pass "image loaded: vector table at $VECTORS matches the ELF" \
                                  || fail "vector table at $VECTORS reads $(rd "$VECTORS"), expected $want"
a=$(rd $counter); sleep 1; b=$(rd $counter)
if [[ $((0x$b)) -gt $((0x$a)) ]]; then
  pass "program runs from AXISRAM: counter @$counter 0x$a -> 0x$b"
else
  sed 's/^/      /' /tmp/gdbsrv.log | tail -15
  fail "counter @$counter did not advance (0x$a -> 0x$b)"
fi
echo "smoke: all checks passed"
EOF
}

cmd_clock() {
  local secs="${1:-60}"
  [[ "$secs" =~ ^[0-9]+$ ]] || die "usage: n6.sh clock [seconds-per-source]"
  N6_SECS="$secs" in_container <<'EOF'
. /work/n6/lib.sh
cd /tmp
arm-none-eabi-as -march=armv8.1-m.main -mthumb -o smoke.o /work/n6/smoke/smoke.S
arm-none-eabi-ld -T /work/n6/smoke/smoke.ld -o smoke.elf smoke.o
load_and_start smoke.elf >/dev/null   # keep the core running so CYCCNT counts
python /work/n6/clock.py "$N6_SECS" 2> >(grep -v -i "disk devices by id" >&2)
EOF
}

cmd_run() {
  local elf="${1:-}"
  [[ -f "$elf" ]] || die "usage: n6.sh run <ram-linked.elf>"
  local abs; abs="$(cd "$(dirname "$elf")" && pwd)/$(basename "$elf")"
  N6_EXTRA_MOUNT="$abs:/work/app.elf:ro" in_container <<'EOF'
. /work/n6/lib.sh
load_and_start /work/app.elf
EOF
}

cmd_shell() {
  ensure_image
  cmd_attach >/dev/null
  docker run --rm -it --privileged -v /dev/bus/usb:/dev/bus/usb \
    -v "$N6_DIR:/work/n6:ro" -e CHIP="$CHIP" "$IMAGE" bash
}

case "${1:-}" in
  attach) cmd_attach ;;
  detach) cmd_detach ;;
  probe)  cmd_probe ;;
  smoke)  cmd_smoke ;;
  clock)  shift; cmd_clock "$@" ;;
  run)    shift; cmd_run "$@" ;;
  shell)  cmd_shell ;;
  *) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
