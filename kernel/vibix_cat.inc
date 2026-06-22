; void cat(int argc, const char **argv)
; Reads from stdin (fd=0) in 256-byte chunks, writes to stdout (fd=1).
; Takes argc/argv for interface consistency (ignores them — VIBIX has no open()).
cat:
    push rbx
    push r12
    sub rsp, 256                ; 256-byte read buffer on stack
    mov r12, rsp                ; r12 = buffer pointer

.loop:
    mov rax, 2                  ; SYS_READ
    xor rdi, rdi                ; fd = 0 (stdin)
    mov rsi, r12                ; buf
    mov rdx, 256                ; len
    syscall

    test rax, rax
    jle .done                   ; EOF or error

    mov rdx, rax                ; bytes read → write length
    mov rax, 1                  ; SYS_WRITE
    mov rdi, 1                  ; fd = 1 (stdout)
    mov rsi, r12                ; buf
    syscall

    jmp .loop

.done:
    add rsp, 256
    pop r12
    pop rbx
    ret
