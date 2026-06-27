;==============================================================================
; syscall_entry.asm — SYSCALL/SYSRET entry with per-process kernel stack
;
; Uses current_proc_kernel_rsp (updated by scheduler on each switch)
; instead of a dedicated global syscall stack.
;
; After syscall_handler returns, checks should_schedule flag.
; If set (process exited or blocked), builds synthetic interrupt-compatible
; frame and switches through the scheduler.  Otherwise returns via sysretq.
;==============================================================================

bits 64
section .text

;------------------------------------------------------------------------------
; Externals
;------------------------------------------------------------------------------

extern syscall_handler
extern scheduler_switch_exit

;------------------------------------------------------------------------------
; Per-process globals (updated by scheduler)
;------------------------------------------------------------------------------

section .data
align 8

global current_proc_kernel_rsp
current_proc_kernel_rsp: dq 0

global syscall_state
syscall_state:
    .rsp:    dq 0       ; +0: user RSP
    .rflags: dq 0       ; +8: user RFLAGS
    .rip:    dq 0       ; +16: user RIP (RCX on SYSCALL)

global should_schedule
should_schedule: db 0

;------------------------------------------------------------------------------
; Syscall entry point
;------------------------------------------------------------------------------

section .text
global syscall_entry
syscall_entry:
    ; Save user RSP
    mov [rel syscall_state.rsp], rsp

    ; Switch to per-process kernel stack
    mov rsp, [rel current_proc_kernel_rsp]

    ; Save user RIP (RCX) and RFLAGS (R11)
    mov [rel syscall_state.rip], rcx
    mov [rel syscall_state.rflags], r11

    ; ── Set up C ABI call: syscall_handler(num, arg1, arg2, arg3, arg4) ──
    ; After SYSCALL: rdi=arg1, rsi=arg2, rdx=arg3, r8=arg4, rax=num
    ; C ABI:         rdi=num,  rsi=arg1, rdx=arg2, rcx=arg3, r8=arg4
    mov r9, rdi          ; r9 = user arg1 (safe)
    mov rcx, rdx         ; rcx = arg3 = user rdx
    mov rdx, rsi         ; rdx = arg2 = user rsi
    mov rsi, r9          ; rsi = arg1 = user rdi
    mov rdi, rax         ; rdi = num = syscall number



    call syscall_handler

    ; Check for pending reschedule
    cmp byte [rel should_schedule], 1
    je .exit_or_block

    ; ── Normal return via sysretq ──
    mov rcx, [rel syscall_state.rip]
    mov r11, [rel syscall_state.rflags]
    mov rsp, [rel syscall_state.rsp]
    db 0x48, 0x0f, 0x07    ; sysretq

;------------------------------------------------------------------------------
; Exit / block path — divert through scheduler
;------------------------------------------------------------------------------
; Build a synthetic interrupt-compatible frame from the saved syscall_state,
; then call scheduler_switch_exit which returns the next process's kernel_rsp.

.exit_or_block:
    ; Clear flag BEFORE building frame (avoid recursive entry)
    mov byte [rel should_schedule], 0

    ; Discard the call return address from syscall_handler call
    add rsp, 8

    ; Build frame HIGH→LOW (matching irq_common pop order).
    ; Individual pushes in reverse order: r15 first (highest), rax last (RSP).

    ; iretq frame (highest addresses)
    push 0x1B                       ; SS
    push qword [rel syscall_state.rsp]  ; user RSP
    push 0x202                      ; RFLAGS (IF enabled)
    push 0x23                       ; CS (user code | 3)
    push qword [rel syscall_state.rip]  ; RIP

    ; err_code + int_no
    push 0                          ; err_code
    push 0                          ; int_no

    ; GPRs (reverse push: r15 first, rax last = RSP)
    push 0                          ; R15
    push 0                          ; R14
    push 0                          ; R13
    push 0                          ; R12
    push 0                          ; R11
    push 0                          ; R10
    push 0                          ; R9
    push 0                          ; R8
    push 0                          ; RDI
    push 0                          ; RSI
    push 0                          ; RBP
    push 0                          ; RBX
    push 0                          ; RDX
    push 0                          ; RCX
    push 0                          ; RAX  ← RSP now points here

    ; RSP now points at RAX — pass as argument
    mov rdi, rsp
    call scheduler_switch_exit
    mov rsp, rax

    ; Pop GPRs (irq_common order: rax..r15)
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

    add rsp, 16    ; skip int_no + err_code
    iretq
