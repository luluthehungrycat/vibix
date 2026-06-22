; void clear_cmd(int argc, const char **argv)
; Clear the terminal using ANSI escape codes: ESC[2J + ESC[H
clear_cmd:
    push rbx
    push r12
    push r13

    lea r12, [rel clear_seq]
    mov rdx, 4                  ; len = ESC[2J (4 bytes)
    mov rsi, r12
    mov rdi, 1                  ; STDOUT
    mov rax, 1                  ; SYS_WRITE
    syscall

    lea r13, [rel home_seq]
    mov rdx, 3                  ; len = ESC[H (3 bytes)
    mov rsi, r13
    mov rdi, 1
    mov rax, 1
    syscall

    pop r13
    pop r12
    pop rbx
    ret

; String data
clear_seq: db 0x1B, "[2J"
home_seq:  db 0x1B, "[H"
