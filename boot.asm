;==============================================================================
; boot.asm — Multiboot v1 bootloader (ELF32) + 64-bit kernel wrapper
;
; QEMU's -kernel loads a Multiboot-compliant ELF32 image.  This file:
;   1. Provides a Multiboot v1 header (ELF32).
;   2. Handles the 32→64 bit long-mode transition.
;   3. Embeds the 64-bit kernel binary (kernel64.bin) via incbin.
;
; The 64-bit Rust kernel is compiled separately and linked as a flat binary,
; then stitched in here as a .kernel64 section so the ELF32 loader places it
; at 0x200000 — exactly where the transition code jumps to.
;==============================================================================

bits 32
section .text

; ── Multiboot v1 header ────────────────────────────────────────────────────
MB1_MAGIC       equ 0x1BADB002
MB1_FLAGS       equ 0x00000003     ; bit 0 = align, bit 1 = mem map
MB1_CHECKSUM    equ -(MB1_MAGIC + MB1_FLAGS)

align 4
mb1_header:
    dd MB1_MAGIC
    dd MB1_FLAGS
    dd MB1_CHECKSUM

; ── Entry point ────────────────────────────────────────────────────────────
global _start
_start:
    ; Save Multiboot info pointer at a fixed physical address (below 1 MB)
    ; so the 64-bit Rust kernel can read it later.
    mov [0x5000], ebx

    ; ── Build 4-level page tables (identity-map first 2 MB) ───────────────
    mov edi, page_table_l4
    mov cr3, edi

    xor eax, eax
    mov ecx, 0x3000 >> 2
    cld
    rep stosd

    mov edi, cr3
    lea eax, [page_table_l3 + 0x7]
    mov [edi], eax

    lea edi, [page_table_l3]
    lea eax, [page_table_l2 + 0x7]
    mov [edi], eax

    lea edi, [page_table_l2]
    mov eax, 0x83                       ; P | RW | PS (2 MiB huge page)
    mov ecx, 2                          ; Map first 4 MiB (2 × 2 MiB pages)
.lp:
    mov [edi], eax
    add edi, 8
    add eax, 0x200000
    loop .lp

    ; ── Enable PAE ─────────────────────────────────────────────────────────
    mov eax, cr4
    or eax, 1 << 5
    mov cr4, eax

    ; ── Enable Long Mode ───────────────────────────────────────────────────
    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr

    ; ── Enable Paging ──────────────────────────────────────────────────────
    mov eax, cr0
    or eax, 1 << 31
    mov cr0, eax

    ; ── Far-jump into 64-bit mode ──────────────────────────────────────────
    lgdt [gdt64.pointer]
    jmp gdt64.code:start_64

;==============================================================================
bits 64

start_64:
    mov ax, gdt64.data
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; Jump to the embedded 64-bit C kernel at 0x200000.
    mov rax, 0x200000
    jmp rax

;==============================================================================
section .data
align 16

gdt64:
    dq 0
.code: equ $ - gdt64
    dq (1 << 41) | (1 << 43) | (1 << 44) | (1 << 47) | (1 << 53)
.data: equ $ - gdt64
    dq (1 << 41) | (1 << 44) | (1 << 47)
.pointer:
    dw $ - gdt64 - 1
    dq gdt64

section .bss
align 4096

page_table_l4:
    resb 4096
page_table_l3:
    resb 4096
page_table_l2:
    resb 4096

stack_bottom:
    resb 16384
stack_top:

;==============================================================================
; Embedded 64-bit kernel binary, loaded by the ELF32 loader at 0x200000.
;==============================================================================
section .kernel64 align=4096
    incbin "kernel64.bin"
