/* Hot code stays in ITCM; data/BSS may spill into lower OCRAM. RF calibration
   owns 0x42024000..0x42028000. Stack/diagnostics use OCRAM above that scratch;
   the top 8 KiB (0x2202e000..0x22030000) remain BLE exchange memory. */
OUTPUT_ARCH(riscv)
ENTRY(_flash_start)
PROVIDE(bl706_fatal_trap = bl706_default_trap);
MEMORY {
    HEADER (r) : ORIGIN = 0x21000000, LENGTH = 176
    XIP (rx) : ORIGIN = 0x23000000, LENGTH = 1M - 4K
    /* 48 KiB TCM followed by 16 KiB OCRAM, ending before RF scratch. */
    RAM (rwx) : ORIGIN = 0x22014000, LENGTH = 64K
    STACK (rw) : ORIGIN = 0x22028000, LENGTH = 8K
    DIAGNOSTIC (rw) : ORIGIN = 0x2202a000, LENGTH = 16K
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
        KEEP(*(.init)) KEEP(*(.text.clock_switch)) KEEP(*(.text.trap))
        /* Radio deadlines cannot tolerate repeated single-SPI cache misses. */
        *(.text.rwble_* .text.rwip_* .text.lld_* .text.ea_*)
        *(.text.ble_co_list_* .text.ble_ke_event_* .bleromro .bleromro.*)
        . = ALIGN(4);
    } > RAM
    .vectors : AT(LOADADDR(.init) + ADDR(.vectors) - ADDR(.init)) ALIGN(64) {
        KEEP(*(.vectors))
        . = ALIGN(4); __tcm_end = .;
    } > RAM
    /* Boot ROM maps physical offset 0x1000 to virtual XIP 0x23000000. */
    .text : AT(ADDR(.text) + 0x1000) ALIGN(64) {
        *(.text .text.* .bleromro .bleromro.*)
        . = ALIGN(4);
    } > XIP
    .rodata : AT(ADDR(.rodata) + 0x1000) ALIGN(4) {
        *(.rodata .rodata.* .srodata .srodata.*)
        . = ALIGN(4);
    } > XIP
    .data : AT(ALIGN(LOADADDR(.vectors) + SIZEOF(.vectors), 4)) ALIGN(4) {
        __ocram_start = .; __data_start = .;
        *(.data .data.* .sdata .sdata.* .bleromrw .bleromrw.* .retention .retention.* .tcm_data .tcm_data.*)
        . = ALIGN(4); __data_end = .; __ocram_end = .;
    } > RAM
    PROVIDE(__global_pointer$ = __data_start + 0x800);
    __tcm_load_xip = LOADADDR(.init) - 0x1000;
    __ocram_load_xip = LOADADDR(.data) - 0x1000;
    .bss (NOLOAD) : ALIGN(4) {
        __bss_start = .;
        *(.bss .bss.* .sbss .sbss.* .retention_noinit .retention_noinit.* COMMON)
        . = ALIGN(4); __bss_end = .;
    } > RAM
    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .; . += 8K; __stack_top = .;
    } > STACK
    /* init_channel explicitly clears this opt-in, all-zero trace buffer. */
    .pd_trace (NOLOAD) : ALIGN(4) { KEEP(*(.pd_trace)) } > DIAGNOSTIC
    __rf_scratch_start = 0x22024000;
    __rf_scratch_end = 0x22028000;
    /DISCARD/ : { *(.eh_frame .eh_frame_hdr) }
}
ASSERT(SIZEOF(.bootheader) == 176, "BL702 boot header must be 176 bytes")
ASSERT(__tcm_end <= 0x22020000, "BLE hot code overflowed ITCM")
ASSERT(__bss_end <= __rf_scratch_start, "BLE data overlaps RF scratch")
ASSERT(__stack_top <= ORIGIN(STACK) + LENGTH(STACK), "BLE stack overlaps RF calibration")
ASSERT(ADDR(.pd_trace) + SIZEOF(.pd_trace) <= 0x2202e000, "PD trace overlaps exchange memory")
ASSERT(LOADADDR(.data) + SIZEOF(.data) <= 0x23100000, "flash overflow")
ASSERT((bl706_vectors & 63) == 0, "CLIC vector table must be 64-byte aligned")
ASSERT((bl706_fatal_trap & 63) == 0, "CLIC mtvec BASE must be 64-byte aligned")
ASSERT(LOADADDR(.vectors) - LOADADDR(.init) == ADDR(.vectors) - ADDR(.init), "TCM copy layout mismatch")
ASSERT(LOADADDR(.text) - ADDR(.text) == 0x1000, "text XIP mapping mismatch")
ASSERT(LOADADDR(.rodata) - ADDR(.rodata) == 0x1000, "rodata XIP mapping mismatch")
