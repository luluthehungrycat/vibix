;==============================================================================
; user_init.asm — PID 1 init process using ported echo command
;
; Replaces the original raw-write init with the GVIBU-ported echo command.
; Echo is implemented via the shared vibix_echo.inc library, which supports
; -n, -e, -E, -- flags and escape decoding (\n, \t, \r, \\, \0NNN).
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

args_hello: dq cmd_name, str_hello
args_from:  dq cmd_name, str_from

cmd_name:   db "echo", 0
str_hello:  db "Hello, world!", 0
str_from:   db "From PID 1 (init)", 0

; ── Shared echo implementation ────────────────────────────────────────────────
%include "vibix_echo.inc"
