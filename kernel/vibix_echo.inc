;==============================================================================
; vibix_echo.inc — Shared echo implementation for VIBIX flat binaries
;
; Provides: echo(), strlen(), decode_escapes()
;
; Include at the end of your .rodata section:
;   section .rodata
;   %include "vibix_echo.inc"
;==============================================================================

;==============================================================================
; echo(rdi=argc, rsi=argv[])
;
; Print arguments joined by spaces to stdout, then a newline (unless -n).
; Returns 0 in rax.
;
; Flags -n, -e, -E, -- are consumed from the front of the argument list.
; First non-flag argument ends option processing.
;
; Registers preserved: rbx, r12, r13, r14, r15
; Stack frame: [rsp] = 256-byte decode buffer
;==============================================================================
echo:
    push rbx
    push r12
    push r13
    push r14
    push r15
    sub rsp, 256                        ; [rsp] = decode buffer for escapes

    mov r12d, edi                       ; r12 = argc
    mov r13, rsi                        ; r13 = argv
    xor r14d, r14d                      ; r14 = flags (b0=-n, b1=-e)
    mov r15d, 1                         ; r15 = 1 (skip argv[0] = command name)

; ── Option parsing ──────────────────────────────────────────────────────────
.opt_loop:
    cmp r15, r12
    jae .print_args                     ; no args → skip to printing
    mov rsi, [r13 + r15*8]              ; rsi = pointer to arg string
    cmp byte [rsi], '-'
    jne .print_args                     ; not a flag → done

    ; Check for "--" (end-of-options sentinel)
    cmp byte [rsi+1], '-'
    jne .short_flag
    cmp byte [rsi+2], 0
    jne .print_args                     ; not exactly "--"
    inc r15                             ; consume "--"
    jmp .print_args

.short_flag:
    mov al, [rsi+1]
    cmp al, 'n'
    je .flag_n
    cmp al, 'e'
    je .flag_e
    cmp al, 'E'
    je .flag_E
    jmp .print_args                     ; unrecognized flag → stop parsing

.flag_n:
    or r14b, 1                          ; set no_newline
    inc r15
    jmp .opt_loop

.flag_e:
    or r14b, 2                          ; set enable_escapes
    inc r15
    jmp .opt_loop

.flag_E:
    and r14b, 0xFD                      ; clear enable_escapes
    inc r15
    jmp .opt_loop

; ── Write positional arguments ───────────────────────────────────────────────
.print_args:
    cmp r15, r12
    jae .maybe_newline                  ; no positional args → just newline

.print_loop:
    mov rsi, [r13 + r15*8]              ; rsi = current arg string

    ; Get string length
    mov rdi, rsi
    call strlen
    mov rbx, rax                        ; rbx = raw string length

    test r14b, 2                        ; enable_escapes set?
    jz .write_raw                       ; no → write string as-is

    ; ── Decode escape sequences into stack buffer ─────────────────────────
    mov rdi, rsi                        ; src = arg string
    lea rsi, [rsp]                      ; dst = stack buffer
    mov rdx, 256                        ; max output length
    call decode_escapes
    mov rdx, rax                        ; length for write
    lea rsi, [rsp]                      ; buffer for write
    jmp .write_common

.write_raw:
    mov rdx, rbx                        ; length = raw string length

.write_common:
    push r12                            ; argc
    push r14                            ; flags
    push r15                            ; current index
    push rbx                            ; raw string length

    mov rdi, 1                          ; STDOUT
    mov rax, 1                          ; SYS_WRITE
    syscall

    pop rbx
    pop r15
    pop r14
    pop r12

    ; Advance to next arg
    inc r15
    cmp r15, r12
    jae .maybe_newline

    ; ── Write space separator between arguments ───────────────────────────
    lea rsi, [rel str_space]
    mov rdx, 1

    push r12
    push r14
    push r15
    push rbx

    mov rdi, 1                          ; STDOUT
    mov rax, 1                          ; SYS_WRITE
    syscall

    pop rbx
    pop r15
    pop r14
    pop r12

    jmp .print_loop

; ── Newline (unless -n) ──────────────────────────────────────────────────────
.maybe_newline:
    test r14b, 1                        ; no_newline set?
    jnz .exit_echo

    lea rsi, [rel str_newline]
    mov rdx, 1

    mov rdi, 1                          ; STDOUT
    mov rax, 1                          ; SYS_WRITE
    syscall

.exit_echo:
    xor eax, eax                        ; return 0
    add rsp, 256
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    ret

;==============================================================================
; strlen(rdi = string) → rax = length (excluding null terminator)
;==============================================================================
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
;==============================================================================
; decode_escapes(rdi = src, rsi = dst, rdx = max_len) → rax = decoded length
;
; Decode escape sequences from null-terminated src into dst buffer:
;   \n → 0x0A    \t → 0x09    \r → 0x0D    \\ → 0x5C
;   \0[NNN] → byte with octal value (0-3 octal digits, default 0)
;   \<unknown> → literal backslash followed by the character
;==============================================================================
decode_escapes:
    push rbx
    push r12

    mov r8, rdi                         ; r8 = src pointer
    mov r9, rsi                         ; r9 = dst pointer
    mov r10, rdx                        ; r10 = max output length
    xor r11d, r11d                      ; r11 = decoded byte count

.loop:
    cmp r11, r10
    jae .done
    mov al, [r8]
    test al, al
    jz .done

    cmp al, 0x5C                        ; backslash?
    jne .copy_char

    inc r8                              ; consume backslash
    mov al, [r8]
    test al, al
    jz .lone_backslash

    cmp al, 'n'
    je .esc_nl
    cmp al, 't'
    je .esc_tab
    cmp al, 'r'
    je .esc_cr
    cmp al, 0x5C
    je .esc_bs
    cmp al, '0'
    je .esc_octal

    ; Unknown escape — output backslash then char literally
    mov byte [r9 + r11], 0x5C
    inc r11
    cmp r11, r10
    jae .done
    mov byte [r9 + r11], al
    inc r11
    inc r8
    jmp .loop

.esc_nl:
    mov byte [r9 + r11], 0x0A
    inc r11
    inc r8
    jmp .loop

.esc_tab:
    mov byte [r9 + r11], 0x09
    inc r11
    inc r8
    jmp .loop

.esc_cr:
    mov byte [r9 + r11], 0x0D
    inc r11
    inc r8
    jmp .loop

.esc_bs:
    mov byte [r9 + r11], 0x5C
    inc r11
    inc r8
    jmp .loop

.lone_backslash:
    mov byte [r9 + r11], 0x5C
    inc r11
    inc r8
    jmp .loop

; ── Octal escape \0NNN ──────────────────────────────────────────────────────
.esc_octal:
    inc r8                              ; past '0'
    xor ebx, ebx
    mov r12d, 3                         ; max 3 octal digits

.octal_try:
    mov al, [r8]
    sub al, '0'
    jb .octal_write
    cmp al, 7
    ja .octal_write
    shl ebx, 3
    add ebx, eax
    inc r8
    dec r12d
    jnz .octal_try

.octal_write:
    mov [r9 + r11], bl
    inc r11
    jmp .loop

.copy_char:
    mov [r9 + r11], al
    inc r11
    inc r8
    jmp .loop

.done:
    mov rax, r11
    pop r12
    pop rbx
    ret

; ── String constants (guarded for multi-include) ──────────────────────────────
%ifndef VIBIX_STR_SPACE
%define VIBIX_STR_SPACE
str_space:   db " ", 0
%endif
%ifndef VIBIX_STR_NEWLINE
%define VIBIX_STR_NEWLINE
str_newline: db 0x0A, 0
%endif
