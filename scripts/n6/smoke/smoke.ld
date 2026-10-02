/* Load into AXISRAM2 at ST's development-boot load address (secure alias). */
MEMORY
{
  RAM (rwx) : ORIGIN = 0x34180400, LENGTH = 255K
}

ENTRY(reset)

SECTIONS
{
  .vectors : { KEEP(*(.vectors)) } > RAM
  .text    : { *(.text*) *(.rodata*) } > RAM
  .bss (NOLOAD) : { *(.bss*) } > RAM
  _estack = ORIGIN(RAM) + LENGTH(RAM);
}
