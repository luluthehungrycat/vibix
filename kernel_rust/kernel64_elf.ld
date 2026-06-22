/*==============================================================================
 * kernel64_elf.ld — 64-bit kernel linker script (ELF output)
 *
 * Identical to root kernel64.ld except this outputs ELF64 (so we can objcopy
 * to flat binary later).  Rust staticlib + asm entry get linked at 0x200000.
 *============================================================================*/

OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start64)

SECTIONS
{
    . = 0x200000;

    .text : { *(.text) }
    .rodata : { *(.rodata) }
    .data : { *(.data) }

    .bss : {
        _bss_start = .;
        *(COMMON) *(.bss) *(.bss.*)
    }
    _bss_end = .;
}
