; void true_cmd(int argc, const char **argv)
; Exit with code 0.
true_cmd:
    xor rdi, rdi
    mov rax, 0
    syscall

; void false_cmd(int argc, const char **argv)
; Exit with code 1.
false_cmd:
    mov rdi, 1
    mov rax, 0
    syscall

; void yes_cmd(int argc, const char **argv)
; Repeatedly write "y\n" to stdout until killed.
yes_cmd:
    push rbx
    push r12
    lea r12, [rel str_y_newline]
    mov rdx, 2                  ; len = 2 ("y\n")
.loop:
    mov rsi, r12
    mov rdi, 1                  ; STDOUT
    mov rax, 1                  ; SYS_WRITE
    syscall
    jmp .loop

; String data
str_y_newline: db "y", 0x0A
