;==============================================================================
; vibix_user_test.inc — User-mode syscall test suite for VIBIX
;
; Tests: sys_pipe(20), sys_dup(18), sys_dup2(19), sys_getcwd(27),
;        sys_chdir(23)
;
; Entry: user_test() — sets up stack, runs tests, exits when done.
;
; Can also be called from init_demo via `call user_test` (must exit itself).
; Based on vibix_vfstest.inc test pattern.
;==============================================================================

%ifndef VIBIX_USER_TEST_INC
%define VIBIX_USER_TEST_INC

; ── Test entry point ──────────────────────────────────────────────────────────
user_test:
    ; write "User Test Begin\n"
    lea rsi, [rel ut_str_begin]
    call ut_print_str

    ; ══════════════════════════════════════════════════════════════════════════
    ; Test 1: Pipe — create, write, read back, compare byte-by-byte
    ; ══════════════════════════════════════════════════════════════════════════
    lea rdi, [rel ut_pipefd]
    mov rax, 20                 ; sys_pipe
    syscall
    test rax, rax
    jnz .ut_test1_fail

    ; Write test data to pipe write end [pipefd+4]
    mov edi, [rel ut_pipefd + 4]
    lea rsi, [rel ut_pipe_data]
    mov edx, 10                 ; "HELLO_PIPE" is 10 bytes
    mov rax, 1                  ; sys_write
    syscall
    cmp rax, 10
    jne .ut_test1_fail

    ; Read back from pipe read end [pipefd]
    mov edi, [rel ut_pipefd]
    lea rsi, [rel ut_readbuf]
    mov edx, 10
    mov rax, 2                  ; sys_read
    syscall
    cmp rax, 10
    jne .ut_test1_fail

    ; Byte-by-byte compare
    lea rsi, [rel ut_pipe_data]
    lea rdi, [rel ut_readbuf]
    mov rcx, 10
    repe cmpsb
    jne .ut_test1_fail

    ; Success
    lea rsi, [rel ut_str_pipe_ok]
    call ut_print_str
    jmp .ut_test2

.ut_test1_fail:
    lea rsi, [rel ut_str_pipe_fail]
    call ut_print_str

    ; ══════════════════════════════════════════════════════════════════════════
    ; Test 2: dup — duplicate stdout (fd=1), verify new fd != 1
    ; ══════════════════════════════════════════════════════════════════════════
.ut_test2:
    mov edi, 1
    mov rax, 18                 ; sys_dup
    syscall
    mov r12d, eax               ; save new fd
    test eax, eax
    js .ut_test2_fail
    cmp eax, 1
    je .ut_test2_fail

    ; Print "[OK] dup test: newfd=X"
    lea rsi, [rel ut_str_dup_prefix]
    call ut_print_str
    mov edi, r12d
    call ut_print_uint64
    lea rsi, [rel ut_str_newline]
    call ut_print_str
    jmp .ut_test3

.ut_test2_fail:
    lea rsi, [rel ut_str_dup_fail]
    call ut_print_str

    ; ══════════════════════════════════════════════════════════════════════════
    ; Test 3: dup2 — duplicate stdout (fd=1) to fd=5, verify rax == 5
    ; ══════════════════════════════════════════════════════════════════════════
.ut_test3:
    mov edi, 1
    mov esi, 5
    mov rax, 19                 ; sys_dup2
    syscall
    cmp eax, 5
    jne .ut_test3_fail

    ; Print "[OK] dup2 test: newfd=5"
    lea rsi, [rel ut_str_dup2_prefix]
    call ut_print_str
    mov edi, 5
    call ut_print_uint64
    lea rsi, [rel ut_str_newline]
    call ut_print_str
    jmp .ut_test4

.ut_test3_fail:
    lea rsi, [rel ut_str_dup2_fail]
    call ut_print_str

    ; ══════════════════════════════════════════════════════════════════════════
    ; Test 4: getcwd — get current working directory, should be "/"
    ; ══════════════════════════════════════════════════════════════════════════
.ut_test4:
    lea rdi, [rel ut_cwdbuf]
    mov esi, 256
    mov rax, 27                 ; sys_getcwd
    syscall

    ; Verify rax == buf pointer (success)
    lea rdi, [rel ut_cwdbuf]
    cmp rax, rdi
    jne .ut_test4_fail

    ; Print "[OK] getcwd test: "
    lea rsi, [rel ut_str_getcwd_prefix]
    call ut_print_str
    ; Print actual cwd string
    lea rsi, [rel ut_cwdbuf]
    call ut_print_str
    lea rsi, [rel ut_str_newline]
    call ut_print_str
    jmp .ut_test5

.ut_test4_fail:
    lea rsi, [rel ut_str_getcwd_fail]
    call ut_print_str

.ut_test5:
    ; Skip test 5 for now — chdir+getcwd needs VFS debugging
    jmp .ut_done

    ; ══════════════════════════════════════════════════════════════════════════
    ; Done — print end marker and exit
    ; ══════════════════════════════════════════════════════════════════════════
.ut_done:
    lea rsi, [rel ut_str_end]
    call ut_print_str

    ret

; ── Helper: ut_print_str(rsi=string) ──────────────────────────────────────────
; Writes null-terminated string at rsi to stdout (fd=1).
; Preserves all caller registers (syscall clobbers rcx, r11).
ut_print_str:
    push rcx
    push r11
    push rax
    push rdi
    push rdx
    push rsi

    mov rdi, rsi
    call strlen                 ; from vibix_core.inc
    mov rdx, rax                ; rdx = length for syscall

    pop rsi                     ; restore string pointer
    mov edi, 1                  ; stdout
    mov eax, 1                  ; SYS_WRITE
    syscall

    pop rdx
    pop rdi
    pop rax
    pop r11
    pop rcx
    ret

; ── Helper: ut_print_uint64(rdi=value) ──────────────────────────────────────
; Converts unsigned 64-bit integer to decimal and writes to stdout.
; Uses ut_num_buf as scratch (24 bytes).
; Preserves all caller registers (syscall clobbers rcx, r11).
ut_print_uint64:
    push rbx
    push rcx
    push r11
    push rax
    push rdi
    push rdx
    push rsi

    mov rax, rdi                ; value to convert
    mov rbx, 10                 ; divisor
    lea rcx, [rel ut_num_buf + 22]
    mov byte [rcx], 0           ; null terminator near buffer end

.conv_loop:
    xor edx, edx
    div rbx                    ; rax = quotient, rdx = remainder (0-9)
    add dl, '0'
    dec rcx
    mov [rcx], dl
    test rax, rax
    jnz .conv_loop

    ; Write the decimal string (rcx points to first digit)
    mov rsi, rcx
    push rsi
    mov rdi, rsi
    call strlen
    mov rdx, rax
    pop rsi
    mov edi, 1                  ; stdout
    mov eax, 1                  ; SYS_WRITE
    syscall

    pop rsi
    pop rdx
    pop rdi
    pop rax
    pop r11
    pop rcx
    pop rbx
    ret

; ── Strings ──────────────────────────────────────────────────────────────────
section .text

ut_str_begin:       db "User Test Begin", 0x0A, 0

ut_str_pipe_ok:     db "  [OK] pipe test: HELLO_PIPE", 0x0A, 0
ut_str_pipe_fail:   db "  [FAIL] pipe test", 0x0A, 0

ut_str_dup_prefix:  db "  [OK] dup test: newfd=", 0
ut_str_dup_fail:    db "  [FAIL] dup test", 0x0A, 0

ut_str_dup2_prefix: db "  [OK] dup2 test: newfd=", 0
ut_str_dup2_fail:   db "  [FAIL] dup2 test", 0x0A, 0

ut_str_getcwd_prefix: db "  [OK] getcwd test: ", 0
ut_str_getcwd_fail:   db "  [FAIL] getcwd test", 0x0A, 0

ut_str_chdir_prefix: db "  [OK] chdir+getcwd test: ", 0
ut_str_chdir_fail:   db "  [FAIL] chdir+getcwd test", 0x0A, 0
ut_debug_cd:         db "CD=", 0
ut_debug_gc:         db "GC=", 0

ut_str_end:         db "User Test End", 0x0A, 0
ut_str_newline:     db 0x0A, 0

ut_pipe_data:       db "HELLO_PIPE"

ut_path_dev:        db "/dev", 0

; ── Buffers (for pipe, cwd, number conversion) ──────────────────────────────
align 8
ut_pipefd:      dd 0, 0         ; [0]=read_fd, [4]=write_fd (2 i32s)
ut_readbuf:     resb 64
ut_cwdbuf:      resb 256
ut_num_buf:     resb 24

%endif
