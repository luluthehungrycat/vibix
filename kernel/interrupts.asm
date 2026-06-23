;==============================================================================
; interrupts.asm — 64-bit interrupt service routine stubs
;
; Provides ISR 0-31 (CPU exceptions) + common handler that saves/restores
; registers and calls the Rust interrupt_handler() function.
;
; ISRs with error-code (pushed by CPU): 8, 10, 11, 12, 13, 14, 17, 21
; All others: push a dummy 0 for error code.
;==============================================================================

bits 64
section .text

;------------------------------------------------------------------------------
; ISR stubs — one per CPU exception, for index 0..31
;------------------------------------------------------------------------------

%macro ISR_NOERR 1
global isr%1
isr%1:
    push 0          ; dummy error code (CPU didn't push one)
    push %1         ; interrupt number
    jmp isr_common
%endmacro

%macro ISR_ERR 1
global isr%1
isr%1:
    push %1         ; interrupt number (CPU already pushed error code)
    jmp isr_common
%endmacro

; Exceptions without error code
ISR_NOERR 0     ; Divide-by-zero
ISR_NOERR 1     ; Debug
ISR_NOERR 2     ; Non-maskable Interrupt
ISR_NOERR 3     ; Breakpoint
ISR_NOERR 4     ; Overflow
ISR_NOERR 5     ; Bound Range Exceeded
ISR_NOERR 6     ; Invalid Opcode
ISR_NOERR 7     ; Device Not Available
ISR_NOERR 9     ; Coprocessor Segment Overrun
ISR_NOERR 15    ; Reserved
ISR_NOERR 16    ; x87 FPU Floating-Point Error
ISR_NOERR 18    ; Machine Check
ISR_NOERR 19    ; SIMD Floating-Point Exception
ISR_NOERR 20    ; Virtualization Exception
; 22-31 reserved, all no-error
ISR_NOERR 22
ISR_NOERR 23
ISR_NOERR 24
ISR_NOERR 25
ISR_NOERR 26
ISR_NOERR 27
ISR_NOERR 28
ISR_NOERR 29
ISR_NOERR 30
ISR_NOERR 31

; Exceptions with error code
ISR_ERR 8       ; Double Fault
ISR_ERR 10      ; Invalid TSS
ISR_ERR 11      ; Segment Not Present
ISR_ERR 12      ; Stack-Segment Fault
ISR_ERR 13      ; General Protection Fault
ISR_ERR 14      ; Page Fault
ISR_ERR 17      ; Alignment Check
ISR_ERR 21      ; Control Protection Exception

;------------------------------------------------------------------------------
; IRQ stubs — one per PIC IRQ line (0..15), mapped to vectors 32..47
;------------------------------------------------------------------------------

%macro IRQ 2
global irq%1
irq%1:
    push 0          ; dummy error code
    push %2         ; interrupt vector (32 + IRQ#)
    jmp irq_common
%endmacro

IRQ 0, 32
IRQ 1, 33
IRQ 2, 34
IRQ 3, 35
IRQ 4, 36
IRQ 5, 37
IRQ 6, 38
IRQ 7, 39
IRQ 8, 40
IRQ 9, 41
IRQ 10, 42
IRQ 11, 43
IRQ 12, 44
IRQ 13, 45
IRQ 14, 46
IRQ 15, 47

;------------------------------------------------------------------------------
; IRQ common handler — saves volatile registers, calls Rust handler,
; sends EOI to the master PIC (and slave if needed), calls scheduler,
; restores, iretq.
;------------------------------------------------------------------------------
; Stack layout when irq_common runs:
;   [rsp+0]   = r15              ← pushed last (rsp points here)
;   [rsp+8]   = r14
;   ...
;   [rsp+112] = rax
;   [rsp+120] = int_no
;   [rsp+128] = err_code (dummy 0)
;   [rsp+136] = rip
;   [rsp+144] = cs
;   [rsp+152] = rflags
;   [rsp+160] = user_rsp  (ONLY if interrupt from userspace CPL=3)
;   [rsp+168] = ss         (ONLY if interrupt from userspace CPL=3)
;------------------------------------------------------------------------------

irq_common:
    ; Save all volatile registers
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push rdi
    push rsi
    push rbp
    push rbx
    push rdx
    push rcx
    push rax

    ; First argument = frame pointer (rsp points to saved r15)
    mov rdi, rsp

    ; Call the Rust IRQ handler
    extern irq_handler
    call irq_handler

    ; Send End-Of-Interrupt to the master PIC (port 0x20)
    ; For slave IRQs (8-15), also send to slave PIC (port 0xA0).
    mov al, 0x20        ; EOI value
    out 0x20, al        ; always send to master

    ; Check frame->int_no (it's at rsp + 15*8 + 8 after register push)
    ; After saving regs, int_no is at [rsp + 15*8] = [rsp + 120]
    mov rax, [rsp + 120]
    cmp rax, 40
    jb .eoi_done
    mov al, 0x20
    out 0xA0, al        ; send EOI to slave too
.eoi_done:

    ; Call scheduler_tick — must be after EOI to avoid re-entrancy
    mov rdi, rsp                ; arg: current kernel_rsp (points at RAX)
    extern scheduler_tick
    call scheduler_tick
    mov rsp, rax                ; switch to next process's stack

    ; Restore registers (reverse order)
    pop rax
    pop rcx
    pop rdx
    pop rbx
    pop rbp
    pop rsi
    pop rdi
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15

    ; Clean up int_no and err_code pushed by IRQ stub
    add rsp, 16

    ; Return to interrupted code
    iretq

;------------------------------------------------------------------------------
; Common handler — saves all volatile registers, calls Rust handler,
; restores registers, and returns via iretq.
;------------------------------------------------------------------------------
; Stack layout when isr_common runs:
;   [rsp+0]   = r15              ← pushed last (rsp points here)
;   [rsp+8]   = r14
;   [rsp+16]  = r13
;   [rsp+24]  = r12
;   [rsp+32]  = r11
;   [rsp+40]  = r10
;   [rsp+48]  = r9
;   [rsp+56]  = r8
;   [rsp+64]  = rdi (saved from interrupted context)
;   [rsp+72]  = rsi
;   [rsp+80]  = rbp
;   [rsp+88]  = rbx
;   [rsp+96]  = rdx
;   [rsp+104] = rcx
;   [rsp+112] = rax
;   [rsp+120] = int_no
;   [rsp+128] = err_code (dummy 0 or CPU-pushed)
;   [rsp+136] = rip
;   [rsp+144] = cs
;   [rsp+152] = rflags
;   [rsp+160] = user_rsp  (ONLY if interrupt from userspace CPL=3)
;   [rsp+168] = ss         (ONLY if interrupt from userspace CPL=3)
;------------------------------------------------------------------------------

isr_common:
    ; Save all volatile registers
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push rdi
    push rsi
    push rbp
    push rbx
    push rdx
    push rcx
    push rax

    ; First argument = frame pointer (rsp points to saved r15)
    mov rdi, rsp

    ; Call the Rust handler
    extern interrupt_handler
    call interrupt_handler

    ; Restore registers (reverse order)
    pop rax
    pop rcx
    pop rdx
    pop rbx
    pop rbp
    pop rsi
    pop rdi
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15

    ; Clean up int_no and err_code pushed by ISR stub
    add rsp, 16

    ; Return to interrupted code
    iretq
