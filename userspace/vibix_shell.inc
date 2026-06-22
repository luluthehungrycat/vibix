;==============================================================================
; vibix_shell.inc — Interactive serial shell for VIBIX
;
; Provides: shell() — main entry point (read-eval-print loop)
;
; Depends on: vibix_core.inc (write_str, strlen, write_newline)
;
; Features:
;   - Readline with backspace editing
;   - Command history (4 entries, up/down arrow navigation)
;   - Command table dispatch (echo, cat, printenv, clear, true, false)
;   - Builtins: exit, help, history, clear
;   - Error messages for unknown commands
;
; Include AFTER all other inc files so command labels are visible.
;==============================================================================

; ── Configuration ─────────────────────────────────────────────────────────────
HISTORY_DEPTH   equ 4           ; number of remembered commands
LINE_MAX        equ 256         ; max input line length
MAX_ARGS        equ 16          ; max args per command
%define PROMPT_STR "vibix$ "     ; shell prompt (text macro, used in db below)

; ── Main shell entry point ───────────────────────────────────────────────────
; Called from dispatch table. Never returns (loops until 'exit' builtin).
; Input: rdi = argc, rsi = argv, rdx = envp
shell:
    ; Reset stack to top of user stack page
    mov rsp, 0x2003000

    push rbx
    push r12
    push r13
    push r14
    push r15

    ; Save envp for printenv builtin
    mov r15, rdx                    ; r15 = envp (may be NULL)

    ; Zero-initialize history
    xor eax, eax
    lea rdi, [rel history_buf]
    mov ecx, HISTORY_DEPTH * LINE_MAX
    rep stosb

    ; Initialize history count
    mov qword [rel history_count], 0
    mov qword [rel history_index], 0

    ; ── Welcome banner ──────────────────────────────────────────────────────
    lea rsi, [rel str_welcome]
    mov rdi, 1
    call write_str
    call write_newline
    call write_newline

; ── Main read-eval-print loop ────────────────────────────────────────────────
.shell_loop:
    ; Print prompt: "vibix$ "
    lea rsi, [rel str_prompt]
    mov rdi, 1
    call write_str

    ; ── Read a line with editing and history ────────────────────────────────
    lea rdi, [rel input_line]
    mov esi, LINE_MAX
    call readline
    mov r12, rax                    ; r12 = line length

    ; Empty line → reprompt
    test r12, r12
    jz .shell_loop

    ; Null-terminate the line
    lea rbx, [rel input_line]
    mov byte [rbx + r12], 0

    ; Strip trailing newline/cr if present
    dec r12
    mov al, [rbx + r12]
    cmp al, 0x0A
    je .strip_nl
    cmp al, 0x0D
    je .strip_cr
    inc r12                         ; not a newline, restore
    jmp .parsing

.strip_nl:
.strip_cr:
    mov byte [rbx + r12], 0

.parsing:
    ; ── Parse line into command + args ──────────────────────────────────────
    ; rbx = input_line, r12 = length (adjusted)
    lea rdi, [rel input_line]
    lea rsi, [rel argv_buf]
    mov ecx, MAX_ARGS
    call parse_line
    mov r13, rax                    ; r13 = argc

    test r13, r13
    jz .shell_loop                  ; no command → reprompt

    ; ── Check builtins ──────────────────────────────────────────────────────
    mov rdi, [rel argv_buf]         ; rdi = command name

    ; "exit" builtin
    lea rsi, [rel str_exit]
    call strcmp
    test rax, rax
    jnz .check_help
    jmp .exit_shell

.check_help:
    lea rsi, [rel str_help]
    call strcmp
    test rax, rax
    jnz .check_history
    call shell_help
    jmp .shell_loop

.check_history:
    lea rsi, [rel str_history_cmd]
    call strcmp
    test rax, rax
    jnz .check_clear
    call shell_history
    jmp .shell_loop

.check_clear:
    lea rsi, [rel str_clear]
    call strcmp
    test rax, rax
    jnz .check_empty
    ; "clear" — just run clear_cmd
    mov rdi, 1
    lea rsi, [rel argv_buf]
    call clear_cmd
    jmp .shell_loop

.check_empty:
    ; Empty command name (shouldn't happen)
    cmp byte [rdi], 0
    je .shell_loop

    ; ── Lookup in command table ─────────────────────────────────────────────
    mov rdi, [rel argv_buf]         ; reload rdi (corrupted by builtin checks)
    lea r14, [rel cmd_table]        ; r14 = cmd table pointer
    xor r12d, r12d                  ; r12 = command index

.find_loop:
    mov rsi, [r14]                  ; rsi = table entry name pointer
    test rsi, rsi
    jz .not_found                   ; NULL → end of table

    push rdi                        ; save command name
    push rsi                        ; save table entry name
    call strcmp
    pop rsi
    pop rdi
    test rax, rax
    jnz .next_entry

    ; Match found — call the function
    mov rax, [r14 + 8]              ; rax = function pointer
    mov rdi, r13                    ; rdi = argc
    lea rsi, [rel argv_buf]         ; rsi = argv
    xor edx, edx                    ; rdx = 0 (envp placeholder)
    mov rdx, r15                    ; rdx = envp (for printenv)
    call rax
    jmp .shell_loop

.next_entry:
    add r14, 16
    inc r12d
    jmp .find_loop

.not_found:
    ; Print "command not found" message
    lea rsi, [rel str_not_found_1]
    mov rdi, 1
    call write_str
    mov rsi, [rel argv_buf]
    mov rdi, 1
    call write_str
    lea rsi, [rel str_not_found_2]
    mov rdi, 1
    call write_str
    call write_newline
    jmp .shell_loop

; ── Exit builtin ─────────────────────────────────────────────────────────────
.exit_shell:
    lea rsi, [rel str_bye]
    mov rdi, 1
    call write_str
    call write_newline
    xor edi, edi
    mov eax, 0                          ; SYS_EXIT
    syscall

;==============================================================================
; readline(buf, max_len) — Read line with editing and history
;
; Input:  rdi = buffer address, esi = max length
; Output: rax = line length (0 for empty/EOF)
;
; Features:
;   - Echoes printable characters
;   - Backspace (0x7F or 0x08) deletes last character
;   - Ctrl+D on empty line → exit shell
;   - Up arrow (ESC [ A) / Down arrow (ESC [ B) for history
;   - Enter (0x0D) submits line
;==============================================================================
readline:
    push rbx
    push r12
    push r13
    push r14
    push r15

    mov r12, rdi                    ; r12 = buffer
    mov r13d, esi                   ; r13 = max length
    xor r14d, r14d                  ; r14 = current position (offset in buf)

    ; Reset history browse position to end
    mov r15, [rel history_count]    ; r15 = history browse index

.readline_loop:
    ; Read one byte from stdin
    sub rsp, 16
    mov rdi, 0                      ; fd = stdin
    mov rsi, rsp                    ; buffer on stack
    mov rdx, 1                      ; 1 byte
    mov rax, 2                      ; SYS_READ
    syscall
    add rsp, 16

    test rax, rax
    jle .eof                        ; EOF or error → end of input

    mov al, [rsp - 16]              ; reload the byte we read

    ; ── Handle Enter (0x0D) ────────────────────────────────────────────────
    cmp al, 0x0D
    je .enter

    ; ── Handle Ctrl+D ───────────────────────────────────────────────────────
    cmp al, 0x04
    je .ctrl_d

    ; ── Handle Backspace (0x7F or 0x08) ────────────────────────────────────
    cmp al, 0x7F
    je .backspace
    cmp al, 0x08
    je .backspace

    ; ── Handle Escape sequences (arrow keys) ────────────────────────────────
    cmp al, 0x1B
    je .escape_seq

    ; ── Handle regular printable characters ─────────────────────────────────
    cmp al, 0x20                    ; space
    jb .readline_loop               ; ignore other control chars
    cmp al, 0x7E                    ; DEL (handled above) and beyond
    ja .readline_loop

    ; Check if buffer is full
    cmp r14d, r13d
    jae .readline_loop

    ; Store character
    mov [r12 + r14], al
    inc r14d

    ; Echo character back
    push rax
    mov rdi, 1                      ; STDOUT
    mov rsi, rsp                    ; byte on stack
    mov rdx, 1
    mov rax, 1                      ; SYS_WRITE
    syscall
    pop rax

    jmp .readline_loop

.backspace:
    test r14d, r14d
    jz .readline_loop               ; nothing to delete

    dec r14d                        ; move position back

    ; Send backspace: move left, overwrite with space, move left again
    ; Sequence: 0x08 ' ' 0x08
    push rax
    push rbx
    mov byte [rsp], 0x08
    mov rdi, 1
    mov rsi, rsp
    mov rdx, 1
    mov rax, 1                      ; SYS_WRITE
    syscall

    mov byte [rsp], 0x20            ; space
    mov rax, 1
    syscall

    mov byte [rsp], 0x08            ; backspace again
    mov rax, 1
    syscall
    pop rbx
    pop rax

    jmp .readline_loop

.escape_seq:
    ; Read next two bytes (ESC [ A/B/C/D)
    sub rsp, 16
.read_esc_1:
    mov rdi, 0
    mov rsi, rsp
    mov rdx, 1
    mov rax, 2                      ; SYS_READ
    syscall
    test rax, rax
    jle .esc_done
    mov al, [rsp]
    cmp al, '['                     ; CSI
    jne .esc_done
.read_esc_2:
    mov rdi, 0
    mov rsi, rsp
    mov rdx, 1
    mov rax, 2                      ; SYS_READ
    syscall
    test rax, rax
    jle .esc_done
    mov al, [rsp]
    cmp al, 'A'                     ; Up arrow
    je .history_up
    cmp al, 'B'                     ; Down arrow
    je .history_down
    ; Ignore other arrow keys (C=right, D=left)
.esc_done:
    add rsp, 16
    jmp .readline_loop

.history_up:
    add rsp, 16                     ; clean up esc read stack
    ; Check if there's history to browse
    cmp qword [rel history_count], 0
    je .readline_loop
    ; Decrement browse index (earlier in history)
    test r15d, r15d
    jz .readline_loop               ; already at oldest
    dec r15d
    call history_display
    jmp .readline_loop

.history_down:
    add rsp, 16                     ; clean up esc read stack
    ; Increment browse index (later in history)
    mov rax, [rel history_count]
    cmp r15d, eax
    jae .readline_loop              ; already at newest
    inc r15d
    call history_display
    jmp .readline_loop

.enter:
    ; Echo newline
    push rax
    mov byte [rsp], 0x0A
    mov rdi, 1
    mov rsi, rsp
    mov rdx, 1
    mov rax, 1
    syscall
    pop rax

    ; Null-terminate
    mov byte [r12 + r14], 0

    ; Add non-empty line to history
    test r14d, r14d
    jz .return_line

    push r12
    push r14
    mov rdi, r12
    call history_add
    pop r14
    pop r12

.return_line:
    mov rax, r14                    ; return line length
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    ret

.ctrl_d:
    ; Ctrl+D on empty line → exit
    test r14d, r14d
    jnz .readline_loop              ; not empty, ignore

    ; Send newline and return 0
    push rax
    mov byte [rsp], 0x0A
    mov rdi, 1
    mov rsi, rsp
    mov rdx, 1
    mov rax, 1
    syscall
    pop rax

    xor eax, eax                    ; return 0 (empty line)
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    ret

.eof:
    xor eax, eax
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    ret

; ── History display helper ───────────────────────────────────────────────────
; Called from readline after up/down arrow.  Clears current line and
; displays history entry at index r15.
; Input: r15 = history browse index
;        r12 = current buffer, r14 = current position (destroyed then reset)
;        r13 = max length
history_display:
    push rax
    push rcx
    push rdi
    push rsi
    push rdx

    ; Clear current line from cursor position back to prompt
    ; Move back to prompt position
    mov ecx, r14d                   ; number of chars to erase
    test ecx, ecx
    jz .hd_show

.hd_erase_loop:
    ; Backspace: 0x08, space, 0x08
    mov byte [rsp - 16], 0x08
    mov rdi, 1
    lea rsi, [rsp - 16]
    mov rdx, 1
    mov rax, 1
    syscall

    mov byte [rsp - 16], 0x20
    syscall

    mov byte [rsp - 16], 0x08
    syscall

    dec ecx
    jnz .hd_erase_loop

.hd_show:
    ; Get history entry at index r15
    lea rdi, [rel history_buf]
    mov eax, r15d
    mov ecx, LINE_MAX
    mul ecx
    add rdi, rax                    ; rdi = history entry

    ; Copy history into buffer
    push rdi
    mov rdi, r12                    ; buffer
    pop rsi                         ; history entry
    call strcpy
    mov r14d, eax                   ; r14 = new length

    ; Display the history entry text
    mov rsi, r12
    mov rdi, 1
    call write_str

.hd_done:
    pop rdx
    pop rsi
    pop rdi
    pop rcx
    pop rax
    ret

;==============================================================================
; history_add(line) — Add a line to the command history
;
; Input: rdi = null-terminated string
;==============================================================================
history_add:
    push rbx
    push r12
    push r13
    push r14

    mov r12, rdi                    ; r12 = line
    call strlen
    mov r13, rax                    ; r13 = line length

    ; Don't add duplicate of last entry
    cmp qword [rel history_count], 0
    je .ha_do_add

    ; Get last history entry
    mov rax, [rel history_count]
    dec rax
    mov ecx, LINE_MAX
    mul ecx
    lea r14, [rel history_buf + rax] ; r14 = last entry

    ; Compare with current line
    mov rdi, r12
    mov rsi, r14
    call strcmp
    test rax, rax
    jz .ha_done                     ; duplicate, skip

.ha_do_add:
    ; Check if we need to shift (history full)
    mov rax, [rel history_count]
    cmp rax, HISTORY_DEPTH
    jb .ha_no_shift

    ; Shift all entries up (remove oldest)
    mov ecx, (HISTORY_DEPTH - 1) * LINE_MAX
    lea rdi, [rel history_buf]
    lea rsi, [rel history_buf + LINE_MAX]
    rep movsb
    mov rax, HISTORY_DEPTH - 1
    mov [rel history_count], rax

.ha_no_shift:
    ; Append new entry
    mov rax, [rel history_count]
    mov ecx, LINE_MAX
    mul ecx
    lea rdi, [rel history_buf + rax] ; rdi = destination slot

    ; Copy line into history
    mov rsi, r12
    call strcpy

    ; Increment count
    inc qword [rel history_count]

    ; Reset browse position
    mov rax, [rel history_count]
    mov [rel history_index], rax

.ha_done:
    pop r14
    pop r13
    pop r12
    pop rbx
    ret

;------------------------------------------------------------------------------
; history_display_all — Print all history entries (for "history" builtin)
;------------------------------------------------------------------------------
history_display_all:
    push rbx
    push r12
    push r13

    mov r12, [rel history_count]
    test r12, r12
    jz .hda_done

    xor r13d, r13d                  ; index
.hda_loop:
    mov ecx, LINE_MAX
    mov eax, r13d
    mul ecx
    lea rbx, [rel history_buf + rax]

    ; Check for empty slot
    cmp byte [rbx], 0
    je .hda_next

    ; Print "  N  command"
    push r13
    push r12

    mov byte [rsp], ' '
    mov rdi, 1
    mov rsi, rsp
    mov rdx, 1
    mov rax, 1
    syscall

    ; Print index (1-based)
    mov rax, r13
    inc rax
    call print_decimal

    mov byte [rsp], ' '
    syscall

    mov rdi, 1
    mov rsi, rbx
    call write_str
    call write_newline

    pop r12
    pop r13

.hda_next:
    inc r13d
    cmp r13d, r12d
    jb .hda_loop

.hda_done:
    pop r13
    pop r12
    pop rbx
    ret

;==============================================================================
; parse_line(buf, argv, max_args) — Split line into argv array
;
; Input:  rdi = null-terminated input line
;         rsi = argv buffer (array of pointers)
;         ecx = max args
; Output: rax = argc
;
; Splits on whitespace (space, tab).  Handles single and double quotes.
; Destroys buf content (inserts null terminators).
;==============================================================================
parse_line:
    push rbx
    push r12
    push r13
    push r14
    push r15

    mov r12, rdi                    ; r12 = input pointer
    mov r13, rsi                    ; r13 = argv buffer
    mov r14d, ecx                   ; r14 = max args
    xor r15d, r15d                  ; r15 = argc

.pl_skip_ws:
    ; Skip leading whitespace
    mov al, [r12]
    test al, al
    jz .pl_done
    cmp al, ' '
    je .pl_skip_char
    cmp al, 0x09                    ; tab
    je .pl_skip_char
    jmp .pl_token_start

.pl_skip_char:
    inc r12
    jmp .pl_skip_ws

.pl_token_start:
    ; Check for max args
    cmp r15d, r14d
    jae .pl_done

    ; Store pointer in argv
    mov [r13 + r15*8], r12
    inc r15d

    ; Parse token: handle quoted vs unquoted
    cmp byte [r12], '"'
    je .pl_double_quote
    cmp byte [r12], 0x27            ; single quote
    je .pl_single_quote
    jmp .pl_unquoted

.pl_double_quote:
    inc r12                         ; skip opening "
.dq_loop:
    cmp byte [r12], '"'
    je .dq_close
    cmp byte [r12], 0
    je .pl_done
    inc r12
    jmp .dq_loop
.dq_close:
    mov byte [r12], 0               ; null-terminate at closing "
    inc r12
    jmp .pl_skip_ws

.pl_single_quote:
    inc r12                         ; skip opening '
.sq_loop:
    cmp byte [r12], 0x27
    je .sq_close
    cmp byte [r12], 0
    je .pl_done
    inc r12
    jmp .sq_loop
.sq_close:
    mov byte [r12], 0               ; null-terminate at closing '
    inc r12
    jmp .pl_skip_ws

.pl_unquoted:
    cmp byte [r12], ' '
    je .pl_end_token
    cmp byte [r12], 0x09            ; tab
    je .pl_end_token
    cmp byte [r12], 0
    je .pl_done
    inc r12
    jmp .pl_unquoted

.pl_end_token:
    mov byte [r12], 0               ; null-terminate token
    inc r12
    jmp .pl_skip_ws

.pl_done:
    mov rax, r15                    ; return argc
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    ret

;==============================================================================
; shell_help — Print help text for builtins
;==============================================================================
shell_help:
    push rbx
    push r12

    lea rsi, [rel str_help_text]
    mov rdi, 1
    call write_str

    pop r12
    pop rbx
    ret

;==============================================================================
; shell_history — Print command history (for "history" builtin)
;==============================================================================
shell_history:
    push rbx
    push r12

    call history_display_all

    pop r12
    pop rbx
    ret

;==============================================================================
; strcmp(rdi=s1, rsi=s2) → rax=0 if equal, !=0 otherwise
;==============================================================================
strcmp:
    xor eax, eax
.sc_loop:
    mov al, [rdi]
    mov cl, [rsi]
    sub al, cl
    jnz .sc_done
    test cl, cl
    jz .sc_done
    inc rdi
    inc rsi
    jmp .sc_loop
.sc_done:
    ret

;==============================================================================
; strcpy(rdi=dst, rsi=src) → rax = bytes copied
;==============================================================================
strcpy:
    push rcx
    xor ecx, ecx
.sp_loop:
    mov al, [rsi + rcx]
    mov [rdi + rcx], al
    test al, al
    jz .sp_done
    inc ecx
    jmp .sp_loop
.sp_done:
    mov rax, rcx
    pop rcx
    ret

;==============================================================================
; print_decimal(rax=value) — Print unsigned decimal number to stdout
;==============================================================================
print_decimal:
    push rbx
    push r12
    push r13
    sub rsp, 32                     ; digit buffer

    mov r12, rsp
    add r12, 31
    mov byte [r12], 0               ; null terminator
    mov r13, rsp

.pd_loop:
    xor edx, edx
    mov ecx, 10
    div ecx
    add dl, '0'
    dec r12
    mov [r12], dl
    test eax, eax
    jnz .pd_loop

    ; Write the digits
    push r12
    mov rdi, 1
    mov rsi, r12
    mov rdx, r13
    sub rdx, r12                    ; length = end - start
    mov rax, 1                      ; SYS_WRITE
    syscall
    pop r12

    add rsp, 32
    pop r13
    pop r12
    pop rbx
    ret

;==============================================================================
; Data section
;==============================================================================
; These must come after the code and before the section ends.
; The data is placed in .text for flat binary flatness.

; ── Strings ───────────────────────────────────────────────────────────────────
str_prompt:        db PROMPT_STR, 0
str_welcome:       db "VIBIX Interactive Shell v0.1", 0
str_bye:           db "Goodbye!", 0
str_exit:          db "exit", 0
str_help:          db "help", 0
str_history_cmd:   db "history", 0
str_clear:         db "clear", 0
str_not_found_1:   db "vibix: ", 0
str_not_found_2:   db ": command not found", 0

str_help_text:
    db "Available commands:", 0x0A
    db "  echo [args]     — print arguments", 0x0A
    db "  cat             — copy stdin to stdout (Ctrl+D to exit)", 0x0A
    db "  printenv [var]  — print environment variables", 0x0A
    db "  clear           — clear terminal", 0x0A
    db "  true            — exit with code 0", 0x0A
    db "  false           — exit with code 1", 0x0A
    db "  exit            — exit shell", 0x0A
    db "  help            — show this help", 0x0A
    db "  history         — show command history", 0x0A
    db 0

; ── Command table ────────────────────────────────────────────────────────────
; Entries: dq name_ptr, function_ptr, ... terminated by dq 0
cmd_table:
    dq str_cat_name, cat
    dq str_echo_name, echo
    dq str_printenv_name, printenv
    dq str_clear, clear_cmd
    dq str_true_name, true_func
    dq str_false_name, false_func
    dq 0    ; terminator

str_cat_name:      db "cat", 0
str_echo_name:     db "echo", 0
str_printenv_name: db "printenv", 0
str_true_name:     db "true", 0
str_false_name:    db "false", 0

; ── Buffers ──────────────────────────────────────────────────────────────────
; Allocated after code in the flat binary
align 8
input_line:      resb LINE_MAX     ; current input line
argv_buf:        resq MAX_ARGS     ; parsed argument pointers
history_buf:     resb HISTORY_DEPTH * LINE_MAX  ; circular history buffer
history_count:   resq 1            ; number of entries in history
history_index:   resq 1            ; current browse position
