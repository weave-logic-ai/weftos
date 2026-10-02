/* RAM-only image for the STM32N6 development boot: the debugger loads it into
 * AXISRAM2 (secure alias); scripts/n6.sh starts it. The origin is 4 KiB aligned
 * because cortex-m-rt's full Armv8-M vector table needs a 2 KiB-aligned base.
 * "FLASH" here is just the code region inside AXISRAM2. */
MEMORY
{
  FLASH : ORIGIN = 0x34181000, LENGTH = 252K
  RAM   : ORIGIN = 0x341C0000, LENGTH = 256K
}
