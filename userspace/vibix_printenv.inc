; void printenv(int argc, const char **argv, const char **envp)
;
; Print all or selected environment variables.
;   argc=1 → print all env vars (NAME=value, one per line)
;   argc>1 → print only vars matching argv[1..argc-1]
;
; envp is a null-terminated array of "NAME=value" strings.
printenv:
    push rbx
    push r12
    push r13
    push r14
    push r15

    mov r12, rdi                ; r12 = argc
    mov r13, rsi                ; r13 = argv
    mov r14, rdx                ; r14 = envp

    cmp r12, 1
    jg .filter                  ; argc > 1 → filter mode

    ; ── Print-all mode ────────────────────────────────────────────────────
.print_all:
    mov r15, r14                ; r15 = current envp entry
.loop_all:
    mov rsi, [r15]
    test rsi, rsi
    jz .done
    mov rdi, 1
    call write_str
    call write_newline
    add r15, 8
    jmp .loop_all

    ; ── Filter mode ────────────────────────────────────────────────────────
.filter:
    mov r15, r14                ; r15 = envp cursor
.filter_env_loop:
    mov rsi, [r15]
    test rsi, rsi
    jz .done

    mov rbx, 1                  ; start from argv[1]
.filter_arg_loop:
    cmp rbx, r12
    jge .filter_next_env

    mov rdi, [r13 + rbx*8]      ; rdi = arg name
    mov rsi, [r15]              ; rsi = env string (reload)
    call starts_with_eq
    test rax, rax
    jz .filter_next_arg

    ; Match found — print env line
    mov rsi, [r15]
    mov rdi, 1
    call write_str
    call write_newline
    jmp .filter_next_env

.filter_next_arg:
    inc rbx
    jmp .filter_arg_loop

.filter_next_env:
    add r15, 8
    jmp .filter_env_loop

.done:
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    ret
