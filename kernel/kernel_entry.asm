;==============================================================================
; kernel_entry.asm — 64-bit entry point from boot.asm
;
; Simply calls kernel_main (defined in kernel.c).
; We keep this separate so boot.asm can focus on the mode-transition dance
; and the C entry point can evolve independently.
;==============================================================================

section .text
bits 64

global _start
_start:
    ; Stack was already set up by boot.asm — just hand off.
    extern kernel_main
    call kernel_main

    ; Kernel returned (shouldn't happen) — park the CPU.
.cli_hlt:
    cli
    hlt
    jmp .cli_hlt
