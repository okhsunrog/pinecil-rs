OUTPUT_ARCH(riscv)
ENTRY(_flash_start)
PROVIDE(bl706_fatal_trap = bl706_default_trap);
MEMORY {
    HEADER (r) : ORIGIN = 0x21000000, LENGTH = 176
    XIP (rx) : ORIGIN = 0x23000000, LENGTH = 1M - 4K
    /* Reserve ROM's lower 16 KiB ILM and upper 16 KiB OCRAM. */
    ITCM (rwx) : ORIGIN = 0x22014000, LENGTH = 48K
    OCRAM (rwx) : ORIGIN = 0x22020000, LENGTH = 48K
}
SECTIONS {
    .bootheader : AT(0x23000000) { KEEP(*(.bootheader)) } > HEADER
    .flash_boot : AT(0x23001000) {
        KEEP(*(.flash_boot)) . = ALIGN(4);
    } > XIP
    /* Consume the RAM-only inputs before the general .text wildcard. Place
       their load image after all XIP code/constants, using forward references. */
    .init : AT(ALIGN(LOADADDR(.rodata) + SIZEOF(.rodata), 64)) ALIGN(64) {
        __tcm_start = .;
        KEEP(*(.init)) KEEP(*(.text.clock_switch)) KEEP(*(.text.flash_transaction)) KEEP(*(.text.trap))
        . = ALIGN(4);
    } > ITCM
    .vectors : AT(LOADADDR(.init) + ADDR(.vectors) - ADDR(.init)) ALIGN(64) {
        KEEP(*(.vectors))
        . = ALIGN(4); __tcm_end = .;
    } > ITCM
    /* Boot ROM maps physical offset 0x1000 to virtual XIP 0x23000000. */
    .text : AT(ADDR(.text) + 0x1000) ALIGN(64) {
        *(.text .text.*)
        . = ALIGN(4);
    } > XIP
    .rodata : AT(ADDR(.rodata) + 0x1000) ALIGN(4) {
        *(.rodata .rodata.* .srodata .srodata.*)
        . = ALIGN(4);
    } > XIP
    .data : AT(ALIGN(LOADADDR(.vectors) + SIZEOF(.vectors), 4)) ALIGN(4) {
        __ocram_start = .; __data_start = .;
        *(.data .data.* .sdata .sdata.*)
        . = ALIGN(4); __data_end = .; __ocram_end = .;
    } > OCRAM
    PROVIDE(__global_pointer$ = __data_start + 0x800);
    __tcm_load_xip = LOADADDR(.init) - 0x1000;
    __ocram_load_xip = LOADADDR(.data) - 0x1000;
    .bss (NOLOAD) : ALIGN(4) {
        __bss_start = .;
        *(.bss .bss.* .sbss .sbss.* COMMON)
        . = ALIGN(4); __bss_end = .;
    } > OCRAM
    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .; . += 8K; __stack_top = .;
    } > OCRAM
    /DISCARD/ : { *(.eh_frame .eh_frame_hdr) }
}
ASSERT(SIZEOF(.bootheader) == 176, "BL702 boot header must be 176 bytes")
ASSERT(__stack_top <= ORIGIN(OCRAM) + LENGTH(OCRAM), "OCRAM overflow")
ASSERT(LOADADDR(.data) + SIZEOF(.data) <= 0x23100000, "flash overflow")
ASSERT((bl706_vectors & 63) == 0, "CLIC vector table must be 64-byte aligned")
ASSERT((bl706_fatal_trap & 63) == 0, "CLIC mtvec BASE must be 64-byte aligned")
ASSERT(LOADADDR(.vectors) - LOADADDR(.init) == ADDR(.vectors) - ADDR(.init), "TCM copy layout mismatch")
ASSERT(LOADADDR(.text) - ADDR(.text) == 0x1000, "text XIP mapping mismatch")
ASSERT(LOADADDR(.rodata) - ADDR(.rodata) == 0x1000, "rodata XIP mapping mismatch")
