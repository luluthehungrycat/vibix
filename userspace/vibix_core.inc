; int write_str(int fd, const char *buf)
; Writes a null-terminated string to fd. Returns bytes written.
write_str:
    push rbx
    push r12
    push r13
    push r14
    push r15
    push rdi                        ; save fd
    push rsi                        ; save buf
    mov rdi, rsi                    ; strlen expects buf in rdi
    call strlen
    mov rdx, rax
    pop rsi
    pop rdi
    mov rax, 1          ; SYS_WRITE
    syscall
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    ret

; int strlen(const char *s)  — rdi=s, rax=length
%ifndef VIBIX_STRLEN
%define VIBIX_STRLEN
strlen:
    xor eax, eax
.l:
    cmp byte [rdi + rax], 0
    je .d
    inc rax
    jmp .l
.d:
    ret
%endif

; void write_newline(void)  — writes "\n" to stdout
write_newline:
    push rbx
    push r12
    push r13
    push r14
    push r15
    lea rsi, [rel str_newline]
    mov rdx, 1
    mov rdi, 1          ; STDOUT
    mov rax, 1          ; SYS_WRITE
    syscall
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    ret

; int starts_with_eq(const char *name, const char *env)
;   rdi=name, rsi=env  →  rax=1 if env starts with "name=", else 0
starts_with_eq:
    xor ecx, ecx
.l:
    mov al, [rdi + rcx]
    test al, al
    jz .check_eq
    cmp al, [rsi + rcx]
    jne .no
    inc rcx
    jmp .l
.check_eq:
    cmp byte [rsi + rcx], '='
    jne .no
    mov eax, 1
    ret
.no:
    xor eax, eax
    ret

; String constants (guarded for multi-include)
%ifndef VIBIX_STR_NEWLINE
%define VIBIX_STR_NEWLINE
str_newline: db 0x0A, 0
%endif
