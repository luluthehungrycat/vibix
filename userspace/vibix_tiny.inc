; void true_cmd(int argc, const char **argv)
; Exit with code 0 — entry point for direct dispatch.
true_cmd:
    xor edi, edi
    mov eax, 0
    syscall

; void false_cmd(int argc, const char **argv)
; Exit with code 1 — entry point for direct dispatch.
false_cmd:
    mov edi, 1
    mov eax, 0
    syscall

; int true_func(int argc, const char **argv, const char **envp)
; Callable from a shell — returns 0 (does not exit).
true_func:
    xor eax, eax
    ret

; int false_func(int argc, const char **argv, const char **envp)
; Callable from a shell — returns 1 (does not exit).
false_func:
    mov eax, 1
    ret

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
