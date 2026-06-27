;==============================================================================
; vibix_stat_chdir.inc — stat/fstat/chdir syscall tests for VIBIX
;
; Tests: sys_stat(21), sys_fstat(22), sys_chdir(23)
;
; Entry: stat_chdir_test() — no args, never returns (exits when done)
;
; Include after vibix_vfstest.inc in section .text (uses its print_str helper).
;==============================================================================

%ifndef VIBIX_STAT_CHDIR_INC
%define VIBIX_STAT_CHDIR_INC

; ── Stat struct field offsets (Linux x86_64, 144 bytes) ────────────────────
SC_STAT_DEV      equ 0
SC_STAT_INO      equ 8
SC_STAT_NLINK    equ 16
SC_STAT_MODE     equ 24
SC_STAT_SIZE     equ 48
SIZEOF_STAT      equ 144

; ── Test entry point ──────────────────────────────────────────────────────────
stat_chdir_test:
    ; Set up stack
    mov rsp, 0x2003000

    push rbx
    push r12
    push r13
    push r14
    push r15

    ; write "Stat/Chdir Test Begin\n"
    lea rsi, [rel sc_str_begin]
    call print_str

    ; ── Test 1: sys_stat("/") ────────────────────────────────────────────────
    lea rdi, [rel sc_path_root]
    lea rsi, [rel sc_stat_buf]
    xor edx, edx
    xor ecx, ecx
    mov eax, 21
    syscall
    test eax, eax
    js .t1_fail
    lea rsi, [rel sc_str_ok_root]
    call print_str
    jmp .t2

.t1_fail:
    lea rsi, [rel sc_str_fail_root]
    call print_str

    ; ── Test 2: sys_stat("/dev/ttyS0") ───────────────────────────────────────
.t2:
    lea rdi, [rel sc_path_tty]
    lea rsi, [rel sc_stat_buf]
    xor edx, edx
    xor ecx, ecx
    mov eax, 21
    syscall
    test eax, eax
    js .t2_fail
    lea rsi, [rel sc_str_ok_tty]
    call print_str
    jmp .t3

.t2_fail:
    lea rsi, [rel sc_str_fail_tty]
    call print_str

    ; ── Test 3: sys_fstat(0) ─────────────────────────────────────────────────
.t3:
    xor edi, edi
    lea rsi, [rel sc_stat_buf]
    xor edx, edx
    xor ecx, ecx
    mov eax, 22
    syscall
    test eax, eax
    js .t3_fail
    lea rsi, [rel sc_str_ok_fstat]
    call print_str
    jmp .t4

.t3_fail:
    lea rsi, [rel sc_str_fail_fstat]
    call print_str

    ; ── Test 4: sys_chdir("/dev") ────────────────────────────────────────────
.t4:
    lea rdi, [rel sc_path_dev]
    mov eax, 23
    syscall
    test eax, eax
    js .t4_fail
    lea rsi, [rel sc_str_ok_chdir]
    call print_str
    jmp .t5

.t4_fail:
    lea rsi, [rel sc_str_fail_chdir]
    call print_str

    ; ── Test 5: relative stat("ttyS0") after chdir("/dev") ───────────────────
.t5:
    lea rdi, [rel sc_path_rel]
    lea rsi, [rel sc_stat_buf]
    xor edx, edx
    xor ecx, ecx
    mov eax, 21
    syscall
    test eax, eax
    js .t5_fail
    lea rsi, [rel sc_str_ok_rel]
    call print_str
    jmp .t6

.t5_fail:
    lea rsi, [rel sc_str_fail_rel]
    call print_str

    ; ── Test 6: sys_chdir("..") back to root ─────────────────────────────────
.t6:
    lea rdi, [rel sc_path_dotdot]
    mov eax, 23
    syscall
    test eax, eax
    js .t6_fail
    lea rsi, [rel sc_str_ok_dotdot]
    call print_str
    jmp .t7

.t6_fail:
    lea rsi, [rel sc_str_fail_dotdot]
    call print_str

    ; ── Test 7: stat(".") ────────────────────────────────────────────────────
.t7:
    lea rdi, [rel sc_path_dot]
    lea rsi, [rel sc_stat_buf]
    xor edx, edx
    xor ecx, ecx
    mov eax, 21
    syscall
    test eax, eax
    js .t7_fail
    lea rsi, [rel sc_str_ok_dot]
    call print_str
    jmp .done

.t7_fail:
    lea rsi, [rel sc_str_fail_dot]
    call print_str

.done:
    lea rsi, [rel sc_str_end]
    call print_str
    ; exit(0)
    xor edi, edi
    mov eax, 0
    syscall

; ── Strings ──────────────────────────────────────────────────────────────────
section .rodata

sc_path_root:   db "/", 0
sc_path_tty:    db "/dev/ttyS0", 0
sc_path_dev:    db "/dev", 0
sc_path_rel:    db "ttyS0", 0
sc_path_dotdot: db "..", 0
sc_path_dot:    db ".", 0

sc_str_begin:        db "Stat/Chdir Test Begin", 0x0A, 0
sc_str_ok_root:      db "  [OK] stat(/) succeeded", 0x0A, 0
sc_str_fail_root:    db "  [FAIL] stat(/) failed", 0x0A, 0
sc_str_ok_tty:       db "  [OK] stat(/dev/ttyS0) succeeded", 0x0A, 0
sc_str_fail_tty:     db "  [FAIL] stat(/dev/ttyS0) failed", 0x0A, 0
sc_str_ok_fstat:     db "  [OK] fstat(0) succeeded", 0x0A, 0
sc_str_fail_fstat:   db "  [FAIL] fstat(0) failed", 0x0A, 0
sc_str_ok_chdir:     db "  [OK] chdir(/dev) succeeded", 0x0A, 0
sc_str_fail_chdir:   db "  [FAIL] chdir(/dev) failed", 0x0A, 0
sc_str_ok_rel:       db "  [OK] relative stat(ttyS0) succeeded", 0x0A, 0
sc_str_fail_rel:     db "  [FAIL] relative stat(ttyS0) failed", 0x0A, 0
sc_str_ok_dotdot:    db "  [OK] chdir(..) succeeded", 0x0A, 0
sc_str_fail_dotdot:  db "  [FAIL] chdir(..) failed", 0x0A, 0
sc_str_ok_dot:       db "  [OK] stat(.) succeeded", 0x0A, 0
sc_str_fail_dot:     db "  [FAIL] stat(.) failed", 0x0A, 0
sc_str_end:          db "Stat/Chdir Test End", 0x0A, 0

; ── Buffers ──────────────────────────────────────────────────────────────────
section .bss
sc_stat_buf:   resb SIZEOF_STAT

%endif
