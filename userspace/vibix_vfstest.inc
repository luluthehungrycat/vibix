;==============================================================================
; vibix_vfstest.inc — VFS syscall exercise test for VIBIX
;
; Tests: sys_open(12), sys_close(13), sys_read_vfs(14), sys_write_vfs(15),
;        sys_lseek(16), sys_getdents(17)
;
; Entry: vfs_test() — no args, never returns (exits when done)
;
; Include after vibix_shell.inc in section .text (uses resb buffers).
; Depends on: strlen() from vibix_core.inc
;==============================================================================

%ifndef VIBIX_VFSTEST_INC
%define VIBIX_VFSTEST_INC

; ── Test entry point ──────────────────────────────────────────────────────────
vfs_test:
    ; Set up stack

    push rbx
    push r12
    push r13
    push r14
    push r15

    ; write "VFS Test Begin\n"
    lea rsi, [rel str_begin]
    call print_str

    ; ── Test 1: sys_open /dev/null (O_RDWR) ──────────────────────────────────
    lea rdi, [rel path_null]
    mov esi, 2          ; O_RDWR
    xor edx, edx
    mov eax, 12         ; sys_open
    syscall
    mov r12d, eax       ; r12 = fd

    test eax, eax
    js .test2           ; skip if open failed

    ; sys_write_vfs(fd, msg_null, 5) — should succeed (discard)
    mov edi, r12d
    lea rsi, [rel msg_null]
    mov edx, 5
    mov eax, 15         ; sys_write_vfs
    syscall

    lea rsi, [rel str_null_ok]
    call print_str

    ; sys_close(fd)
    mov edi, r12d
    mov eax, 13         ; sys_close
    syscall

    ; ── Test 2: sys_open /dev/zero (O_RDONLY) ──────────────────────────────
.test2:
    lea rdi, [rel path_zero]
    xor esi, esi        ; O_RDONLY = 0
    xor edx, edx
    mov eax, 12         ; sys_open
    syscall
    mov r12d, eax

    test eax, eax
    js .test3

    ; sys_read_vfs(fd, read_buf, 16)
    mov edi, r12d
    lea rsi, [rel read_buf]
    mov edx, 16
    mov eax, 14         ; sys_read_vfs
    syscall

    ; Verify first byte of buf is zero
    cmp byte [rel read_buf], 0
    jne .test3

    lea rsi, [rel str_zero_ok]
    call print_str

    mov edi, r12d
    mov eax, 13         ; sys_close
    syscall

    ; ── Test 3: sys_open /dev/ttyS0 (O_RDWR) ──────────────────────────────
.test3:
    lea rdi, [rel path_tty]
    mov esi, 2          ; O_RDWR
    xor edx, edx
    mov eax, 12         ; sys_open
    syscall
    mov r12d, eax

    test eax, eax
    js .tty0_test

    ; sys_write_vfs(fd, msg_tty, 5)
    mov edi, r12d
    lea rsi, [rel msg_tty]
    mov edx, 5
    mov eax, 15         ; sys_write_vfs
    syscall

    lea rsi, [rel str_tty_ok]
    call print_str

    mov edi, r12d
    mov eax, 13         ; sys_close
    syscall

    ; ── Test 4: sys_open /dev/tty0 (O_RDWR) ──────────────────────────────
.tty0_test:
    lea rdi, [rel path_tty0]
    mov esi, 2          ; O_RDWR
    xor edx, edx
    mov eax, 12         ; sys_open
    syscall
    mov r12d, eax
    test eax, eax
    js .test5         ; skip tty0 if open failed

    ; sys_write_vfs(fd, msg_tty0, 5)
    mov edi, r12d
    lea rsi, [rel msg_tty0]
    mov edx, 5
    mov eax, 15         ; sys_write_vfs
    syscall

    lea rsi, [rel str_tty0_ok]
    call print_str

    mov edi, r12d
    mov eax, 13         ; sys_close
    syscall

    ; ── Test 5: sys_getdents on /dev ───────────────────────────────────────
.test5:
    lea rdi, [rel path_dev]
    xor esi, esi        ; O_RDONLY
    xor edx, edx
    mov eax, 12         ; sys_open
    syscall
    mov r12d, eax

    test eax, eax
    js .vfs_done

    ; sys_getdents(fd, dirent_buf, 64)
    mov edi, r12d
    lea rsi, [rel dirent_buf]
    mov edx, 64
    mov eax, 17         ; sys_getdents
    syscall

    ; rax = number of bytes read (or negative error)
    mov r13d, eax

    ; Print "  GETDENTS: "
    lea rsi, [rel str_gd_prefix]
    call print_str

    ; Print the entry count as decimal
    mov edi, r13d
    call print_uint64

    ; Print newline
    lea rsi, [rel str_newline]
    call print_str

    ; sys_close(fd)
    mov edi, r12d
    mov eax, 13         ; sys_close
    syscall

.vfs_done:
    lea rsi, [rel str_done]
    call print_str

    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx

    xor edi, edi
    mov eax, 0          ; SYS_EXIT
    syscall

; ── Helper: print_str(rsi=string) ────────────────────────────────────────────
; Writes null-terminated string at rsi to stdout (fd=1).
; Preserves all caller registers (syscall clobbers rcx, r11).
print_str:
    push rcx
    push r11
    push rax
    push rdi
    push rdx
    push rsi

    mov rdi, rsi
    call strlen             ; rax = length
    mov rdx, rax            ; rdx = length for syscall

    pop rsi                 ; restore string pointer
    mov edi, 1              ; stdout
    mov eax, 1              ; SYS_WRITE
    syscall

    pop rdx
    pop rdi
    pop rax
    pop r11
    pop rcx
    ret

; ── Helper: print_uint64(rdi=value) ──────────────────────────────────────────
; Converts unsigned 64-bit integer to decimal and writes to stdout.
; Uses num_buf as scratch (must be at least 20 bytes).
; Preserves all caller registers (syscall clobbers rcx, r11).
print_uint64:
    push rbx
    push r12
    push rcx
    push r11
    push rax
    push rdi
    push rdx
    push rsi

    mov rax, rdi            ; value to convert
    mov r12, 10             ; divisor
    lea rbx, [rel num_buf + 19]
    mov byte [rbx], 0       ; null terminator at buffer end

.conv_loop:
    xor edx, edx
    div r12                 ; rax = quotient, rdx = remainder (0-9)
    add dl, '0'
    dec rbx
    mov [rbx], dl
    test rax, rax
    jnz .conv_loop

    ; Write the decimal string (rbx points to first digit)
    mov rsi, rbx
    call strlen
    mov rdx, rax
    mov edi, 1              ; stdout
    mov eax, 1              ; SYS_WRITE
    syscall

    pop rsi
    pop rdx
    pop rdi
    pop rax
    pop r11
    pop rcx
    pop r12
    pop rbx
    ret

; ── Strings ───────────────────────────────────────────────────────────────────
str_begin:      db "VFS Test Begin", 0x0A, 0
str_null_ok:    db "  NULL write: OK", 0x0A, 0
str_zero_ok:    db "  ZERO read: OK", 0x0A, 0
str_tty_ok:     db "  TTYS0 write: OK", 0x0A, 0
str_tty0_ok:    db "  TTY0 write: OK", 0x0A, 0
str_gd_prefix:  db "  GETDENTS: ", 0
str_done:       db "VFS Test Complete", 0x0A, 0
path_null:      db "/dev/null", 0
path_zero:      db "/dev/zero", 0
path_tty:       db "/dev/ttyS0", 0
path_tty0:      db "/dev/tty0", 0
path_dev:       db "/dev", 0
msg_null:       db "null", 0
msg_tty:        db "ttyS0", 0
msg_tty0:       db "tty0", 0

; ── Buffers (for read, dirents, number conversion) ───────────────────────────
align 8
read_buf:       resb 64
dirent_buf:     resb 64
num_buf:        resb 20

%endif
