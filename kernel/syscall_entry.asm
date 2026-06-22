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

section .text
global syscall_entry
syscall_entry:
    ; Save user stack pointer to scratch area
    mov [rel syscall_saved_rsp], rsp

    ; Switch to kernel syscall stack
    mov rsp, [rel syscall_kernel_rsp]

    ; Save user RIP (RCX on SYSCALL entry) and RFLAGS (R11)
    push rcx                    ; [rsp+8] = user RIP
    push r11                    ; [rsp]   = user RFLAGS

    ; ─────────────────────────────────────────────────────────────────────────
    ; Set up C ABI call: syscall_handler(num, arg1, arg2, arg3, arg4)
    ;
    ; After SYSCALL:
    ;   rdi = user arg1, rsi = user arg2, rdx = user arg3, r8 = user arg4
    ;   rcx = user RIP (clobbered by SYSCALL — saved on stack)
    ;   rax = syscall number
    ;
    ; We need C ABI registers:
    ;   rdi = num, rsi = arg1, rdx = arg2, rcx = arg3, r8 = arg4
    ;
    ; So rotate: arg1(rsi)←usr_rdi, arg2(rdx)←usr_rsi, arg3(rcx)←usr_rdx
    ; while preserving usr_rdi before clobber and using usr_rsi/usr_rdx correctly.
    ; ─────────────────────────────────────────────────────────────────────────
    mov r9, rdi                 ; r9 = user arg1 (safe, 6th C ABI slot unused)
    mov rcx, rdx                ; rcx = arg3 = user rdx  (C ABI 4th arg)
    mov rdx, rsi                ; rdx = arg2 = user rsi  (C ABI 3rd arg)
    mov rsi, r9                 ; rsi = arg1 = user rdi  (C ABI 2nd arg)
    mov rdi, rax                ; rdi = num = syscall no (C ABI 1st arg)
    ; r8 already holds user arg4 → C ABI 5th arg ✓

    call syscall_handler

    ; Restore return frame
    pop r11                     ; user RFLAGS
    pop rcx                     ; user RIP

    ; Restore user stack pointer
    mov rsp, [rel syscall_saved_rsp]

    ; Return to ring 3
    ; NASM 2.x doesn't recognize sysretq — use explicit encoding
    db 0x48, 0x0f, 0x07          ; sysretq (REX.W + SYSRET)

;------------------------------------------------------------------------------
; Dedicated 4 KB stack for syscall processing
;------------------------------------------------------------------------------

section .bss
align 16
syscall_stack_bottom:
    resb 4096
syscall_stack_top:
