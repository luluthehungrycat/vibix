;==============================================================================
; vibix_signal_test.inc — User-mode signal test suite
;
; Tests: custom handler for SIGUSR1, SIG_IGN for SIGUSR1
;
; Entry: sig_test() — sets up stack, installs handlers, self-kills, reports.
;
; Reuses ut_print_str and ut_print_uint64 from vibix_user_test.inc.
; Must be included after vibix_user_test.inc due to ut_print_* deps.
;==============================================================================

%ifndef VIBIX_SIGNAL_TEST_INC
%define VIBIX_SIGNAL_TEST_INC

; ── SigAction struct layout ───────────────────────────────────────────────────
struc SigAction
    .handler: resq 1
    .mask:    resq 1
    .flags:   resq 1
endstruc

; ── Test entry point ──────────────────────────────────────────────────────────
sig_test:
    ; Debug: write using a known-good string from user_test's .rodata
    lea rsi, [rel ut_str_begin]    ; "User Test Begin\n"
    call ut_print_str
    ; Now try our own string with direct syscall
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel st_str_begin]
    mov rdx, 18
    syscall

    ; ── Test 1: Custom handler for SIGUSR1 ──────────────────────────────────
    ; Build sigaction struct with handler address
    lea rax, [rel st_handler]
    mov qword [rel st_action], rax
    mov qword [rel st_action + 8], 0
    mov qword [rel st_action + 16], 0

    ; sigaction(SIGUSR1=10, &st_action, NULL)
    mov rax, 29
    mov rdi, 10
    lea rsi, [rel st_action]
    xor rdx, rdx
    syscall

    ; kill(1=self, SIGUSR1) — sets sig_pending
    mov rax, 28
    mov rdi, 1
    mov rsi, 10
    syscall

    ; Trigger delivery with a dummy syscall (any number works)
    ; The signal check in syscall_handler will see the pending signal and
    ; route through the scheduler, which delivers to the custom handler.
    mov rax, 31     ; unregistered syscall — harmless
    syscall

    ; After handler + sigreturn, resumed here with RAX=0 (from 0-frame)
    test rax, rax
    jnz .test1_fail
    lea rsi, [rel st_str_usr1_ok]
    mov rax, 1
    mov rdi, 1
    mov rdx, 26
    syscall
    jmp .test2

.test1_fail:
    lea rsi, [rel st_str_usr1_fail]
    mov rax, 1
    mov rdi, 1
    mov rdx, 28
    syscall

    ; ── Test 2: SIG_IGN for SIGUSR1 ─────────────────────────────────────────
.test2:
    ; Build SIG_IGN sigaction (handler=1)
    mov qword [rel st_ignore_action], 1   ; SIG_IGN
    mov qword [rel st_ignore_action + 8], 0
    mov qword [rel st_ignore_action + 16], 0

    ; sigaction(SIGUSR1=10, &st_ignore_action, NULL)
    mov rax, 29
    mov rdi, 10
    lea rsi, [rel st_ignore_action]
    xor rdx, rdx
    syscall

    ; kill(1=self, SIGUSR1) — sets sig_pending
    mov rax, 28
    mov rdi, 1
    mov rsi, 10
    syscall

    ; Dummy syscall to trigger signal check — SIG_IGN should clear the bit
    mov rax, 31
    syscall

    ; If we're here, signal was ignored (process survived)
    lea rsi, [rel st_str_ignore_ok]
    call ut_print_str

    ; ── Done ─────────────────────────────────────────────────────────────────
.done:
    lea rsi, [rel st_str_end]
    call ut_print_str

    ret

; ── Signal Handler for SIGUSR1 ────────────────────────────────────────────────
; Called with RDI = signal number, RSP = base of signal frame.
; All other registers are whatever they were when the signal interrupted.
; Must end with sys_sigreturn (syscall 30) — do NOT return via ret.
st_handler:
    ; Save rdi (signal number) — we'll use it in our message
    push rdi

    ; Write "[SIGNAL] caught SIGUSR1\n"
    mov rax, 1          ; SYS_write
    mov rdi, 1          ; fd = stdout
    lea rsi, [rel st_str_caught]
    mov rdx, st_str_caught_len
    syscall

    pop rdi             ; rdi = signal number (for printing below, optional)

    ; Call sigreturn to restore original context
    mov rax, 30         ; SYS_sigreturn
    syscall
    ; Never returns from sigreturn

; ── Strings (in .text to avoid flat binary section issues) ──────────────────
section .text

st_str_begin:      db "Signal Test Begin", 0x0A, 0
st_str_usr1_ok:    db "  [OK] custom handler test", 0x0A, 0
st_str_usr1_fail:  db "  [FAIL] custom handler test", 0x0A, 0
st_str_ignore_ok:  db "  [OK] ignore test", 0x0A, 0
st_str_caught:     db "  [SIGNAL] caught SIGUSR1", 0x0A
st_str_caught_len: equ $ - st_str_caught
st_str_end:        db "Signal Test End", 0x0A, 0

; ── SigAction buffers ─────────────────────────────────────────────────────────
section .text
align 8
st_action:
            istruc SigAction
    at SigAction.handler, dq 0   ; filled at runtime
    at SigAction.mask,    dq 0
    at SigAction.flags,   dq 0
            iend

align 8
st_ignore_action:
            istruc SigAction
    at SigAction.handler, dq 0   ; filled at runtime with 1
    at SigAction.mask,    dq 0
    at SigAction.flags,   dq 0
            iend

%endif
