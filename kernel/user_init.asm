;==============================================================================
; user_init.asm — PID 1 init process using ported GVIBU echo command
;
; Demonstrates GVIBU echo features: basic output, -e escape decoding,
; -n no-newline, octal escapes. Uses the shared vibix_echo.inc library.
;
; Produces the following serial output:
;   Hello, world!
;   From PID 1 (init)
;   Escapes:    OCTAL:Athere
;   PID 1 (no trailing newline)
;   [empty line]
;
; All key strings expected by test_kernel.py are preserved:
;   "Hello, world!"  "From PID 1 (init)"  "VIBIX: init exited with code"
;
; Syscall ABI (from kernel):
;   rax = syscall number
;   rdi = arg1, rsi = arg2, rdx = arg3, r8  = arg4, r9  = arg5
;   Return value in rax.
;   All registers clobbered except rcx, r11.
;
; Syscall numbers:
;   0 = exit(int code)
;   1 = write(int fd, buf, len)
;   2 = read(int fd, buf, len)
;   3 = getpid()
;   4 = brk(size) — not implemented
;==============================================================================

ORG 0x2000000
bits 64

section .text
global _start

_start:
    ; Stack starts at top of stack page (0x2002000)
    mov rsp, 0x2002000

    ; echo "Hello, world!"
    mov rdi, 2
    lea rsi, [rel args_hello]
    call echo

    ; echo "From PID 1 (init)"
    mov rdi, 2
    lea rsi, [rel args_from]
    call echo

    ; echo -e "Escapes:\tOCTAL:\0101there"
    ; Demonstrates: -e flag, \t (tab), \0NNN (octal escape)
    mov rdi, 3
    lea rsi, [rel args_escapes]
    call echo

    ; echo -n "PID 1"
    ; Demonstrates: -n flag (suppress trailing newline)
    mov rdi, 3
    lea rsi, [rel args_pid]
    call echo

    ; echo (just newline — to terminate the -n line cleanly)
    ; Also demonstrates: echo with no positional args
    mov rdi, 1
    lea rsi, [rel args_just_cmd]
    call echo

    ; getpid() → rax (should be 1 for PID 1)
    mov rax, 3
    syscall

    ; exit(0)
    xor rdi, rdi
    mov rax, 0
    syscall

; ── Halt loop (belt-and-suspenders) ──────────────────────────────────────────
.halt:
    hlt
    jmp .halt

section .rodata

; ── Argument pointer arrays ─────────────────────────────────────────────────
args_hello:     dq cmd_name, str_hello
args_from:      dq cmd_name, str_from
args_escapes:   dq cmd_name, str_e_flag, str_escapes_demo
args_pid:       dq cmd_name, str_n_flag, str_pid
args_just_cmd:  dq cmd_name

; ── String constants ─────────────────────────────────────────────────────────
cmd_name:           db "echo", 0
str_hello:          db "Hello, world!", 0
str_from:           db "From PID 1 (init)", 0
str_e_flag:         db "-e", 0
str_n_flag:         db "-n", 0
str_escapes_demo:   db "Escapes:\tOCTAL:\0101there", 0  ; literal escape sequences
str_pid:            db "PID 1", 0

; ── Shared echo implementation ────────────────────────────────────────────────
%include "vibix_echo.inc"
