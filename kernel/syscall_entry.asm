;==============================================================================
; syscall_entry.asm — SYSCALL/SYSRET entry point for 64-bit long mode
;
; Called via the LSTAR MSR when SYSCALL is executed in ring 3.
; Saves user registers, switches to kernel stack, and dispatches to Rust.
;==============================================================================

bits 64
section .text

;------------------------------------------------------------------------------
; Externals
;------------------------------------------------------------------------------

extern syscall_handler

;------------------------------------------------------------------------------
; Kernel stack area for syscall processing
;------------------------------------------------------------------------------

section .data
align 16

global syscall_kernel_rsp
syscall_kernel_rsp: dq syscall_stack_top

global syscall_saved_rsp
syscall_saved_rsp: dq 0

;------------------------------------------------------------------------------
; Syscall entry point — set LSTAR to this address
;------------------------------------------------------------------------------

global syscall_entry
syscall_entry:
    ; Save user stack pointer to scratch area
    mov [rel syscall_saved_rsp], rsp

    ; Switch to kernel syscall stack
    mov rsp, [rel syscall_kernel_rsp]

    ; Save user RIP (RCX on SYSCALL entry) and RFLAGS (R11)
    push rcx                    ; [rsp+8] = user RIP
    push r11                    ; [rsp]   = user RFLAGS

    ; rdi = syscall number (RAX)
    mov rdi, rax

    ; rsi = arg1, rdx = arg2, r8 = arg3, r9 = arg4
    ; (these are already set from the calling convention)

    call syscall_handler

    ; Restore return frame
    pop r11                     ; user RFLAGS
    pop rcx                     ; user RIP

    ; Restore user stack pointer
    mov rsp, [rel syscall_saved_rsp]

    ; Return to ring 3
    sysretq

;------------------------------------------------------------------------------
; Dedicated 4 KB stack for syscall processing
;------------------------------------------------------------------------------

section .bss
align 16
syscall_stack_bottom:
    resb 4096
syscall_stack_top:
