;==============================================================================
; kernel64_entry.asm — 64-bit entry stub for the embedded kernel binary
;
; boot.asm transitions to long mode and jumps to 0x200000, where this code
; lives.  We zero BSS (being careful not to clobber the 32-bit ELF's page
; tables located at 0x201000+), set up our own stack, and call kernel_main.
;==============================================================================

bits 64
section .text

global _start64

; BSS bounds — exported by kernel64.ld
extern _bss_start
extern _bss_end

_start64:
    ; Switch to our own 16 KB stack (lives past kernel BSS, safe from the
    ; 32-bit ELF's page tables at 0x201000).
    mov rsp, stack_top

    ; Hand off to C — individual modules zero their own BSS as needed.
    extern kernel_main
    call kernel_main

    ; In case kernel_main returns.
.halt:
    cli
    hlt
    jmp .halt

;==============================================================================
; Stack (16 KB) — lives beyond the BSS range, safe from page-table collision.
;==============================================================================
section .bss
align 16
stack_bottom:
    resb 16384
stack_top:
