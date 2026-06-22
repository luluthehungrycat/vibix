;==============================================================================
; userspace_blob.asm — Combined userspace binary with dispatch table
;
; All 8 GVIBU-ported commands in one flat binary.  The kernel selects which
; command to run by setting rdi = command_id before entering user mode.
;
; Command IDs:
;   0 = init_demo   — boot-time init: echo-based system info demo
;   1 = echo_demo   — echo (default: say hello)
;   2 = true_cmd    — exit(0)
;   3 = false_cmd   — exit(1)
;   4 = cat_demo    — stdin→stdout copy
;   5 = printenv    — print environment variables
;   6 = clear_demo  — ANSI ESC[2J ESC[H
;   7 = yes_cmd     — infinite "y\n" loop
;==============================================================================

ORG 0x2000000
bits 64

NUM_COMMANDS equ 8

section .text
global _start

_start:
    ; rdi = command_id (set by kernel before iretq)
    cmp rdi, NUM_COMMANDS
    jb .valid
    xor edi, edi                    ; out-of-range → default to init_demo
.valid:
    lea rax, [rel dispatch_table]
    jmp [rax + rdi*8]

; ── Init demo (PID 1) ─────────────────────────────────────────────────────────
; Produces "Hello, world!\n" and "From PID 1 (init)\n" for test compatibility,
; then demonstrates echo -e with octal escapes.
init_demo:
    mov rsp, 0x2002000

    ; echo "Hello, world!"
    mov rdi, 2
    lea rsi, [rel args_hello]
    call echo

    ; echo -e "\0101there"  (octal 0101 = 'A')
    mov rdi, 3
    lea rsi, [rel args_e_octal]
    call echo

    ; echo "From PID 1 (init)"
    mov rdi, 2
    lea rsi, [rel args_from]
    call echo

    ; getpid()
    mov rax, 3
    syscall

    ; exit(0)
    xor edi, edi
    mov eax, 0
    syscall

; ── Echo demo ─────────────────────────────────────────────────────────────────
echo_demo:
    mov rsp, 0x2002000
    mov rdi, 2
    lea rsi, [rel args_hello]
    call echo
    xor edi, edi
    mov eax, 0
    syscall

; ── Cat demo ──────────────────────────────────────────────────────────────────
cat_demo:
    mov rsp, 0x2002000
    call cat
    xor edi, edi
    mov eax, 0
    syscall

; ── Printenv demo ─────────────────────────────────────────────────────────────
printenv_demo:
    mov rsp, 0x2002000
    mov rdi, 1                      ; argc=1 → print all
    xor rsi, rsi                    ; argv = NULL
    ; rdx = envp — kernel doesn't set this yet, will be NULL → no output
    call printenv
    xor edi, edi
    mov eax, 0
    syscall

; ── Clear demo ────────────────────────────────────────────────────────────────
clear_demo:
    mov rsp, 0x2002000
    call clear_cmd
    xor edi, edi
    mov eax, 0
    syscall

section .rodata

; ── Dispatch table ──────────────────────────────────────────────────────────
dispatch_table:
    dq init_demo        ; 0: PID 1 init (default)
    dq echo_demo        ; 1: echo hello world
    dq true_cmd         ; 2: exit(0)
    dq false_cmd        ; 3: exit(1)
    dq cat_demo         ; 4: stdin→stdout copy
    dq printenv_demo    ; 5: print environment
    dq clear_demo       ; 6: clear terminal (ANSI)
    dq yes_cmd          ; 7: infinite y loop

; ── String data ──────────────────────────────────────────────────────────────
str_echo:       db "echo", 0
str_hello:      db "Hello, world!", 0
str_from:       db "From PID 1 (init)", 0
str_e_flag:     db "-e", 0
str_octal_test: db "\0101there", 0       ; literal backslash-0-1-0-1

; ── Argument arrays ──────────────────────────────────────────────────────────
args_hello:     dq str_echo, str_hello
args_from:      dq str_echo, str_from
args_e_octal:   dq str_echo, str_e_flag, str_octal_test

; ── Include shared implementations ──────────────────────────────────────────
%include "vibix_core.inc"
%include "vibix_tiny.inc"
%include "vibix_echo.inc"
%include "vibix_cat.inc"
%include "vibix_printenv.inc"
%include "vibix_clear.inc"
