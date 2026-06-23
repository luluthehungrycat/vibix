;==============================================================================
; context_switch.asm — Context switch to a process via its saved register frame
;
; Called from Rust: context_switch_to(kernel_rsp)
;
; kernel_rsp points at saved RAX in the register frame (built by
; build_init_frame or saved by scheduler_tick).
;
; Frame layout (matching irq_common pop order):
;   [RSP+0]   = RAX
;   [RSP+8]   = RCX
;   [RSP+16]  = RDX
;   [RSP+24]  = RBX
;   [RSP+32]  = RBP
;   [RSP+40]  = RSI
;   [RSP+48]  = RDI
;   [RSP+56]  = R8
;   [RSP+64]  = R9
;   [RSP+72]  = R10
;   [RSP+80]  = R11
;   [RSP+88]  = R12
;   [RSP+96]  = R13
;   [RSP+104] = R14
;   [RSP+112] = R15
;   [RSP+120] = int_no
;   [RSP+128] = err_code
;   [RSP+136] = RIP
;   [RSP+144] = CS
;   [RSP+152] = RFLAGS
;   [RSP+160] = user RSP
;   [RSP+168] = SS
;==============================================================================

bits 64
section .text

global context_switch_to
context_switch_to:
    ; rdi = kernel_rsp — switch stack
    mov rsp, rdi

    ; Pop GPRs (matching irq_common push order: r15..rax)
    pop rax
    pop rcx
    pop rdx
    pop rbx
    pop rbp
    pop rsi
    pop rdi
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15

    ; Skip int_no + err_code
    add rsp, 16

    ; Return to user mode
    iretq
