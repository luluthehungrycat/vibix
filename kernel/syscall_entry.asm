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
%ifdef DEBUG
extern serial_puts, serial_print_hex8, serial_print_hex64
%endif

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

global sigreturn_pending
sigreturn_pending: db 0

; 18 qwords for sigreturn: [0..14]=RAX..R15, [15]=RIP, [16]=userRSP, [17]=RFLAGS
global sigreturn_frame
sigreturn_frame:
    times 18 dq 0

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
    ; Zero user-space argument registers to avoid leaking kernel/old-process
    ; pointers to the restored process (critical after exec()).
    xor edi, edi
    xor esi, esi
    xor edx, edx
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

    ; GPRs — check sigreturn frame first
    cmp byte [rel sigreturn_pending], 0
    je .normal_push

    ; Restore saved GPRs from sigreturn_frame
    mov byte [rel sigreturn_pending], 0
    ; Push in reverse order: R15 first (highest address), RAX last (RSP points here)
    ; Push order: R15, R14, ..., RAX
    push qword [rel sigreturn_frame + (14*8)]   ; R15
    push qword [rel sigreturn_frame + (13*8)]   ; R14
    push qword [rel sigreturn_frame + (12*8)]   ; R13
    push qword [rel sigreturn_frame + (11*8)]   ; R12
    push qword [rel sigreturn_frame + (10*8)]   ; R11
    push qword [rel sigreturn_frame + (9*8)]    ; R10
    push qword [rel sigreturn_frame + (8*8)]    ; R9
    push qword [rel sigreturn_frame + (7*8)]    ; R8
    push qword [rel sigreturn_frame + (6*8)]    ; RDI
    push qword [rel sigreturn_frame + (5*8)]    ; RSI
    push qword [rel sigreturn_frame + (4*8)]    ; RBP
    push qword [rel sigreturn_frame + (3*8)]    ; RBX
    push qword [rel sigreturn_frame + (2*8)]    ; RDX
    push qword [rel sigreturn_frame + (1*8)]    ; RCX
    push qword [rel sigreturn_frame + (0*8)]    ; RAX  ← RSP now points here

    ; Also restore iretq frame fields (RIP, RFLAGS, user RSP) from
    ; sigreturn_frame[15..17].  The iretq frame was pre-built at lines 111-115
    ; with syscall_state values; overwrite them now.
    ; Offsets from RSP (points at RAX):
    ;   +136 = RIP, +152 = RFLAGS, +160 = user RSP
    mov rcx, [rel sigreturn_frame + (15*8)]    ; saved RIP
    mov [rsp + 136], rcx
    mov rcx, [rel sigreturn_frame + (17*8)]    ; saved RFLAGS
    mov [rsp + 152], rcx
    mov rcx, [rel sigreturn_frame + (16*8)]    ; saved user RSP
    mov [rsp + 160], rcx

    jmp .after_push

.normal_push:
    ; Original all-zeros push
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
.after_push:
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

%ifdef DEBUG
    ;--- DEBUG: print iretq frame SS if != 0x1B ---
    push rax
    push rcx
    push rdx
    push rsi
    mov rax, [rsp + 80]
    cmp al, 0x1B
    je .sys_skip_dbg
    lea rsi, [rel .sys_dbg_excl]
    call serial_puts
    mov rax, [rsp + 80]
    call serial_print_hex8
    lea rsi, [rel .sys_dbg_rsp]
    call serial_puts
    mov rax, rsp
    add rax, 32
    call serial_print_hex64
    lea rsi, [rel .sys_dbg_nl]
    call serial_puts
.sys_skip_dbg:
    pop rsi
    pop rdx
    pop rcx
    pop rax
    jmp .sys_dbg_end
.sys_dbg_excl:  db "!SYS SS=", 0
.sys_dbg_rsp:   db " RSP=0x", 0
.sys_dbg_nl:    db 0x0D, 0x0A, 0
.sys_dbg_end:
%endif

    add rsp, 16    ; skip int_no + err_code

    iretq
