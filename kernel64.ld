OUTPUT_FORMAT(binary)
ENTRY(_start64)

SECTIONS
{
    . = 0x200000;
    .text : { *(.text) }
    .rodata : { *(.rodata) }
    .data : { *(.data) }

    _bss_start = .;
    .bss : { *(COMMON) *(.bss) }
    _bss_end = .;
}
