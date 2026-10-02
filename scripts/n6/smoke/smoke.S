/* N6 smoke program: proves code loaded into AXISRAM executes on the M55.
 * It counts up forever at `counter`; the host reads it twice and expects
 * the value to advance. No peripherals, clocks or flash are touched. */
        .syntax unified
        .arch   armv8.1-m.main
        .thumb

        .section .vectors, "a"
        .word   _estack
        .word   reset

        .text
        .thumb_func
        .global reset
reset:
        ldr     r1, =counter
        movs    r0, #0
1:      adds    r0, r0, #1
        str     r0, [r1]
        b       1b

        .bss
        .align  2
        .global counter
counter:
        .space  4
