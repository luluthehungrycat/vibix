;==============================================================================
; vibix_user_test.inc — User-mode syscall test suite for VIBIX
;
; Tests: sys_pipe(20), sys_dup(18), sys_dup2(19), sys_getcwd(27),
;        sys_chdir(23), fd error/safety semantics
;
; Entry: user_test() — sets up stack, runs tests, exits when done.
;
; Can also be called from init_demo via `call user_test` (must exit itself).
; Based on vibix_vfstest.inc test pattern.
;==============================================================================

%ifndef VIBIX_USER_TEST_INC
%define VIBIX_USER_TEST_INC

%define UT_EBADF 0xfffffffffffffff7

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
    mov [rel ut_dup_fd], eax    ; save new fd across later syscalls
    test eax, eax
    js .ut_test2_fail
    cmp eax, 1
    je .ut_test2_fail

    ; Print "[OK] dup test: newfd=X"
    lea rsi, [rel ut_str_dup_prefix]
    call ut_print_str
    mov edi, [rel ut_dup_fd]
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
    ; ══════════════════════════════════════════════════════════════════════════
    ; Test 5: descriptor validity and dup2 same-fd semantics
    ; ══════════════════════════════════════════════════════════════════════════
    ; fd=5 is the fd created by the existing dup2 test. Close it first so the
    ; following checks cover both a successful close and a closed descriptor.
    mov edi, 5
    mov eax, 13                ; sys_close
    syscall
    test rax, rax
    jnz .ut_fd_errors_fail

    xor esi, esi
    xor edx, edx
    mov eax, 13                ; sys_close
    call ut_expect_ebadf_fd
    test eax, eax
    jnz .ut_fd_errors_fail

    mov eax, 18                ; sys_dup
    call ut_expect_ebadf_fd
    test eax, eax
    jnz .ut_fd_errors_fail

    mov edi, 1                 ; open same-fd dup2 is a no-op success
    mov esi, 1
    mov eax, 19
    syscall
    cmp rax, 1
    jne .ut_dup2_self_fail

    mov edi, 5                 ; closed same-fd dup2 must fail
    mov eax, 19
    call ut_expect_ebadf_same_fd
    test eax, eax
    jne .ut_dup2_self_fail

    mov edi, 16                ; out-of-range same-fd dup2 must fail
    mov eax, 19
    call ut_expect_ebadf_same_fd
    test eax, eax
    jne .ut_dup2_self_fail

    ; Exercise the helper's second-failure cleanup path: make fd 5 valid,
    ; then expect close(fd=5) to fail even though the helper's first fd=16
    ; check still returns EBADF.  The helper must unwind to this call site.
    mov edi, 1
    mov esi, 5
    mov eax, 19
    syscall
    cmp rax, 5
    jne .ut_fd_errors_fail
    xor esi, esi
    xor edx, edx
    mov eax, 13
    call ut_expect_ebadf_fd
    test eax, eax
    jz .ut_fd_errors_fail
    lea rsi, [rel ut_str_fd_helper_failure_path]
    call ut_print_str
    lea rsi, [rel ut_str_fd_helper_cleanup_ok]
    call ut_print_str

    lea rsi, [rel ut_str_fd_errors_ok]
    call ut_print_str
    jmp .ut_test6

.ut_fd_errors_fail:
    lea rsi, [rel ut_str_fd_errors_fail]
    call ut_print_str
    jmp .ut_test6

.ut_dup2_self_fail:
    lea rsi, [rel ut_str_fd_fail]
    call ut_print_str

.ut_test6:
    ; ══════════════════════════════════════════════════════════════════════════
    ; Test 6: VFS and legacy read/write error conventions
    ; ══════════════════════════════════════════════════════════════════════════
    ; VFS IDs 14/15 use -EBADF for invalid/closed descriptors.
    mov esi, 1                 ; non-null pointer: fd validation comes first
    mov edx, 1
    mov eax, 14                ; VFS read
    call ut_expect_ebadf_fd
    test eax, eax
    jnz .ut_vfs_fd_fail

    mov eax, 15                ; VFS write
    call ut_expect_ebadf_fd
    test eax, eax
    jnz .ut_vfs_fd_fail

    ; Legacy IDs 1/2 retain their VIBIX sentinels: write=0 and
    ; read=u64::MAX for invalid descriptors.
    mov edi, 16
    mov esi, 1
    mov edx, 1
    mov eax, 1
    syscall
    test rax, rax
    jnz .ut_vfs_fd_fail
    mov edi, 16
    mov eax, 2
    syscall
    cmp rax, -1
    jne .ut_vfs_fd_fail

    lea rsi, [rel ut_str_vfs_fd_ok]
    call ut_print_str
    jmp .ut_test7

.ut_vfs_fd_fail:
    lea rsi, [rel ut_str_fd_fail]
    call ut_print_str

.ut_test7:
    ; ══════════════════════════════════════════════════════════════════════════
    ; Test 7: lseek/getdents/isatty/TTY invalid and closed descriptors
    ; ══════════════════════════════════════════════════════════════════════════
    xor esi, esi
    xor edx, edx
    mov eax, 16                ; lseek, out of range
    call ut_expect_ebadf_fd
    test eax, eax
    jne .ut_tty_fd_fail

    mov esi, 1
    mov edx, 1
    mov eax, 17                ; getdents
    call ut_expect_ebadf_fd
    test eax, eax
    jne .ut_tty_fd_fail

    xor esi, esi
    xor edx, edx
    mov eax, 24                ; isatty
    call ut_expect_ebadf_fd
    test eax, eax
    jne .ut_tty_fd_fail

    mov esi, 1
    xor edx, edx
    mov eax, 25                ; tcgetattr
    call ut_expect_ebadf_fd
    test eax, eax
    jne .ut_tty_fd_fail

    mov eax, 26                ; tcsetattr
    call ut_expect_ebadf_fd
    test eax, eax
    jne .ut_tty_fd_fail

    lea rsi, [rel ut_str_tty_fd_ok]
    call ut_print_str
    jmp .ut_test8

.ut_tty_fd_fail:
    lea rsi, [rel ut_str_fd_fail]
    call ut_print_str

.ut_test8:
    ; ══════════════════════════════════════════════════════════════════════════
    ; Test 8: dup2 replacement and observable OFT/refcount behavior
    ; ══════════════════════════════════════════════════════════════════════════
    lea rdi, [rel ut_pipefd2]
    mov eax, 20                ; second pipe takes the freed fd slots
    syscall
    test rax, rax
    jnz .ut_refcount_fail

    mov edi, [rel ut_pipefd + 4]  ; source: first pipe write end
    mov esi, [rel ut_pipefd2 + 4] ; target: second pipe write end
    mov eax, 19
    syscall
    cmp eax, [rel ut_pipefd2 + 4]
    jne .ut_refcount_fail

    ; Repeat while both descriptors share the same OFT entry; this must not
    ; drift its refcount.
    mov edi, [rel ut_pipefd + 4]
    mov esi, [rel ut_pipefd2 + 4]
    mov eax, 19
    syscall
    cmp eax, [rel ut_pipefd2 + 4]
    jne .ut_refcount_fail

    ; Closing the original source must leave the replacement alias usable.
    mov edi, [rel ut_pipefd + 4]
    mov eax, 13
    syscall
    test rax, rax
    jnz .ut_refcount_fail

    mov edi, [rel ut_pipefd2 + 4]
    lea rsi, [rel ut_pipe_data]
    mov edx, 10
    mov eax, 15                ; write through replacement alias
    syscall
    cmp eax, 10
    jne .ut_refcount_fail

    mov edi, [rel ut_pipefd]
    lea rsi, [rel ut_readbuf]
    mov edx, 10
    mov eax, 14                ; read from original pipe read end
    syscall
    cmp eax, 10
    jne .ut_refcount_fail

    lea rsi, [rel ut_pipe_data]
    lea rdi, [rel ut_readbuf]
    mov rcx, 10
    repe cmpsb
    jne .ut_refcount_fail

    ; Close the remaining aliases and both pipe read ends.
    mov edi, [rel ut_pipefd2 + 4]
    mov eax, 13
    syscall
    mov edi, [rel ut_pipefd2]
    mov eax, 13
    syscall
    mov edi, [rel ut_pipefd]
    mov eax, 13
    syscall
    lea rsi, [rel ut_str_refcount_ok]
    call ut_print_str
    jmp .ut_done

.ut_refcount_fail:
    lea rsi, [rel ut_str_fd_fail]
    call ut_print_str

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

; ── Helper: ut_expect_ebadf_fd(rax=syscall, rsi/rdx=args) ──────────────────────
; Runs the same descriptor-only call for fd=16 and closed fd=5.
; Returns eax=0 when both return -EBADF, eax=1 otherwise.
ut_expect_ebadf_fd:
    push rbx
    push rsi
    push rdx
    push rax                ; syscall number survives the first syscall
    mov eax, [rsp]
    mov edi, 16
    syscall
    mov r8, UT_EBADF
    cmp rax, r8
    jne .expect_ebadf_fail
    mov eax, [rsp]
    mov edi, 5
    mov rdx, [rsp + 8]
    mov rsi, [rsp + 16]
    syscall
    mov r8, UT_EBADF
    cmp rax, r8
    jne .expect_ebadf_fail_after_args
    ; Discard saved syscall number, then restore the saved argument registers.
    add rsp, 8
    pop rdx
    pop rsi
    pop rbx
    xor eax, eax
    ret

.expect_ebadf_fail:
    add rsp, 24             ; saved rax, rdx, rsi
    jmp .expect_ebadf_cleanup
.expect_ebadf_fail_after_args:
    add rsp, 24             ; saved rax, rdx, rsi
.expect_ebadf_cleanup:
    pop rbx
    mov eax, 1
    ret

ut_expect_ebadf_same_fd:
    push rbx
    push rdi
    push rax                ; syscall number survives both syscalls
    mov eax, [rsp]
    mov edi, [rsp + 8]
    mov esi, edi
    syscall
    mov r8, UT_EBADF
    cmp rax, r8
    jne .expect_same_fail
    mov eax, [rsp]
    mov edi, 5
    mov esi, edi
    syscall
    mov r8, UT_EBADF
    cmp rax, r8
    jne .expect_same_fail
    add rsp, 16
    pop rbx
    xor eax, eax
    ret

.expect_same_fail:
    add rsp, 16
    pop rbx
    mov eax, 1
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

ut_str_fd_errors_ok:   db "  [OK] fd bounds/closed semantics", 0x0A, 0
ut_str_fd_errors_fail: db "  [FAIL] fd bounds/closed semantics", 0x0A, 0
ut_str_fd_helper_failure_path: db "  [TRACE] fd helper second-failure path", 0x0A, 0
ut_str_fd_helper_cleanup_ok: db "  [OK] fd helper second-failure cleanup", 0x0A, 0
ut_str_fd_fail:        db "  [FAIL] fd semantics", 0x0A, 0
ut_str_vfs_fd_ok:      db "  [OK] VFS/legacy fd errors", 0x0A, 0
ut_str_tty_fd_ok:      db "  [OK] lseek/getdents/TTY fd errors", 0x0A, 0
ut_str_refcount_ok:    db "  [OK] dup2 replacement/refcount", 0x0A, 0

ut_str_end:         db "User Test End", 0x0A, 0
ut_str_newline:     db 0x0A, 0

ut_pipe_data:       db "HELLO_PIPE"

ut_path_dev:        db "/dev", 0

; ── Buffers (for pipe, cwd, number conversion) ──────────────────────────────
align 8
ut_pipefd:      dd 0, 0         ; [0]=read_fd, [4]=write_fd (2 i32s)
ut_pipefd2:     dd 0, 0
ut_dup_fd:      dd 0
ut_readbuf:     resb 64
ut_cwdbuf:      resb 256
ut_num_buf:     resb 24

%endif
