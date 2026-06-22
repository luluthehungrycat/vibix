;==============================================================================
; user_init.asm — PID 1 init process (flat binary)
;
; The first user-mode process launched by the kernel. Demonstrates syscalls
; by printing a message and exiting.
;
; Syscall ABI:
;   rax = syscall number
;   rdi = arg1, rsi = arg2, rdx = arg3, r8  = arg4, r9  = arg5
; Return value in rax.
;
; Syscall numbers:
;   0 = exit(int code)          — never returns
;   1 = write(int fd, buf, len) — writes buf to fd, returns bytes written
;   2 = read(int fd, buf, len)  — reads into buf, returns bytes read
;   3 = getpid()                — returns PID
;==============================================================================

bits 64

section .text

global _start
_start:
    ; write(1, msg, len)
    mov rax, 1          ; syscall 1 = write
    mov rdi, 1          ; fd = 1 (stdout)
    lea rsi, [rel msg]  ; buf (RIP-relative addressing works in flat binary)
    mov rdx, 15         ; len
    syscall

    ; write(1, msg2, len2)
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel msg2]
    mov rdx, 17
    syscall

    ; getpid() → rax should be 1
    mov rax, 3
    syscall
    ; rax now contains PID; ignore it for now

    ; exit(0)
    mov rax, 0          ; syscall 0 = exit
    mov rdi, 0          ; code = 0
    syscall

    ; Fallback: halt loop (in case exit returns)
.halt:
    hlt
    jmp .halt

section .rodata
msg:  db "Hello, world!", 0x0A
msg2: db "From PID 1 (init)", 0x0A
