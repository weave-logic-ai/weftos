/* RAM image for the STM32N6 development boot (same window as embassy's N6
 * examples): the boot ROM leaves only the top half of AXISRAM2,
 * [0x34180000, 0x34200000), clocked and CPU-accessible before main() runs, so
 * everything the Reset handler copies or zeroes must stay inside it. */
MEMORY
{
  FLASH : ORIGIN = 0x34180000, LENGTH = 256K
  RAM   : ORIGIN = 0x341C0000, LENGTH = 256K
}
