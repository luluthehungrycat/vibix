;==============================================================================
; vibix_macros.inc — Shared macros and helpers for VIBIX flat binaries
;
; Every flat binary starts with:
;   ORG 0x2000000
;   bits 64
;   section .text
;   global _start
;
; Syscall ABI:
;   rax = syscall number
;   rdi = arg1, rsi = arg2, rdx = arg3, r8  = arg4, r9  = arg5
;   Return value in rax.
;   ALL registers clobbered except rcx, r11 (used by syscall instruction).
;
; Syscall numbers:
;   0 = exit(int code)          — never returns
;   1 = write(int fd, buf, len) — returns bytes written
;   2 = read(int fd, buf, len)  — returns bytes read
;   3 = getpid()                — returns PID
;==============================================================================

; ── Push/pop caller-saved registers (around syscalls) ────────────────────────
; Syscall clobbers rax, rcx, r11, rdi, rsi, rdx, r8, r9, r10.
; These macros save the callee-saved registers that echo uses for state.
; They do NOT preserve rax/rdi/rsi/rdx — those carry syscall parameters.

%macro  push_callee 0
    push rbx
    push r12
    push r13
    push r14
    push r15
%endmacro

%macro  pop_callee 0
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
%endmacro

; ── Syscall convenience wrappers ────────────────────────────────────────────
; Each preserves callee-saved state and returns result in rax.

; void exit(int code)
%macro  exit 1
    mov rax, 0
    mov rdi, %1
    syscall
%endmacro

; int write(int fd, const void *buf, int len)
; Clobbers rcx, r11 and arg regs. Saves/restores rbx,r12-r15.
%macro  write3 3
    push_callee
    mov rax, 1
    mov rdi, %1
    mov rsi, %2
    mov rdx, %3
    syscall
    pop_callee
%endmacro

; str_write — write a null-terminated string
; IN: rdi=fd, rsi=str
; OUT: rax=bytes written
; Clobbers: rdx, rcx, r11
str_write:
    push rbx
    push r12
    push r13
    push r14
    push r15
    push rdi
    push rsi
    call strlen         ; rsi still points to str, rax = len
    mov rdx, rax
    pop rsi
    pop rdi
    mov rax, 1
    syscall
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    ret

; int strlen(const char *s)
; IN: rsi=s
; OUT: rax=length
strlen:
    xor rax, rax
.loop:
    cmp byte [rsi + rax], 0
    je .done
    inc rax
    jmp .loop
.done:
    ret

; void print(const char *str, int fd=1)
; Writes a string to stdout with str_write.
print:
    mov rdi, 1
    ; fall through to str_write
    jmp str_write
