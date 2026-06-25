# VIBIX Multi-Process + Preemptive Scheduler Architecture

> **⚠️ Status: IMPLEMENTED** — This design document describes the Phase 1
> scheduler architecture that has been fully implemented. See the source code
> at `kernel_rust/src/process.rs` and `kernel/context_switch.asm` for the
> actual implementation. Reference this doc for architecture understanding.

## Current State Summary

| Component | Current | Target |
|-----------|---------|--------|
| Processes | Single PID 1 (init) | Up to 64 processes |
| Enter user mode | enter_user_mode() builds iretq frame, never returns | context_switch_to() via scheduler |
| sys_exit | HLT loop (halts entire system) | Kill calling process only, reschedule |
| sys_getpid | Returns hardcoded 1 | Returns current_pid() |
| PIT timer | tick() increments counter | Also triggers context switch |
| Kernel stacks | Single 16KB boot + 4KB dedicated syscall stack | Per-process kernel stacks |
| TSS.RSP0 | Points to boot stack top | Updated on every context switch |
| ELF loader | create_from_elf() dead_code | Used by exec and spawn |
| brk/errno | Global statics | Per-process fields |

---

## 1. Data Structures

### 1.1 Process State

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum ProcessState {
    Ready   = 0,
    Running = 1,
    Blocked = 2,   // waiting for child at wait_for_pid
    Zombie  = 3,
}

pub const MAX_PROCS: usize = 64;
pub const KERNEL_STACK_SIZE: usize = 4096;  // 4 KiB per process

#[repr(C)]
#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u64,
    pub state: ProcessState,

    // --- User-space state ---
    pub entry: u64,              // User RIP / entry point
    pub user_rsp: u64,           // User stack pointer

    // --- Kernel stack ---
    pub kernel_stack_top: u64,   // Top (highest address) of kernel stack page
    pub kernel_rsp: u64,         // Saved RSP --> points at saved rax in reg frame
    pub kernel_stack_base: u64,  // Physical address of the page (for freeing)

    // --- Process relationships ---
    pub parent_pid: u64,         // 0 = no parent (init)
    pub exit_code: u64,          // Set when process becomes Zombie
    pub wait_for_pid: u64,       // 0 = not waiting; else PID being waited for

    // --- Per-process syscall state ---
    pub brk: u64,
    pub errno: i64,

    pub name: [u8; 32],          // Nul-terminated debug name
}
```

### 1.2 Process Table

```rust
pub struct ProcessTable {
    pub slots: [Option<Process>; MAX_PROCS],
    pub next_pid: u64,
    pub count: usize,
}
```

Singleton (only access with interrupts disabled):
```rust
static mut PROCESS_TABLE: ProcessTable = ProcessTable { ... };
static mut CURRENT_PID: u64 = 0;
```

### 1.3 Assembly Externs (in Rust code)

```rust
/// Per-process kernel stack top, updated by scheduler on every switch.
/// Read by syscall_entry.asm to know which stack to use.
extern "C" {
    pub static mut current_proc_kernel_rsp: u64;
}

/// Syscall saved state -- modified by exec to redirect the return.
#[repr(C)]
pub struct SyscallSavedState {
    pub rsp: u64,
    pub rflags: u64,
    pub rip: u64,
}
extern "C" {
    pub static mut syscall_state: SyscallSavedState;
}

/// Flag set by blocking syscalls (exit, waitpid) to force reschedule.
extern "C" {
    pub static mut should_schedule: u8;
}
```

---

## 2. Register Frame Layout (Kernel Stack)

THE CRITICAL CONVENTION. Both the IRQ handler and synthetic init/exec frames must match exactly.

```
HIGH ADDRESS (kernel_stack_top)
  +---------------------------+
  |  SS = 0x1B                |  <-- iretq frame (5 x 8 = 40 bytes)
  |  user RSP                 |
  |  RFLAGS = 0x202           |
  |  CS = 0x23                |
  |  user RIP                 |
  |  err_code = 0             |
  |  int_no = 0               |  <-- 0 for synthetic frames
  |  RAX                      |
  |  RCX                      |
  |  RDX                      |
  |  RBX                      |
  |  RBP                      |
  |  RSI                      |
  |  RDI                      |      saved GPRs (15 x 8 = 120 bytes)
  |  R8                       |
  |  R9                       |
  |  R10                      |
  |  R11                      |
  |  R12                      |
  |  R13                      |
  |  R14                      |
  |  R15                      |  <-- kernel_rsp points HERE
  +---------------------------+
  |   free for C call frames  |
  |   during interrupt        |
  |   handling                |
LOW ADDRESS (kernel_stack_base)
  +---------------------------+
```

**Offset table** (from kernel_rsp):
| Offset | Contents |
|--------|----------|
| +0     | RAX      |
| +8     | RCX      |
| +16    | RDX      |
| +24    | RBX      |
| +32    | RBP      |
| +40    | RSI      |
| +48    | RDI      |
| +56    | R8       |
| +64    | R9       |
| +72    | R10      |
| +80    | R11      |
| +88    | R12      |
| +96    | R13      |
| +104   | R14      |
| +112   | R15      |
| +120   | int_no   |
| +128   | err_code |
| +136   | RIP      |
| +144   | CS       |
| +152   | RFLAGS   |
| +160   | user RSP |
| +168   | SS       |

---

## 3. Context Switch Mechanism

### 3.1 Timer Interrupt Flow

```
PIT IRQ0
  |
  v
CPU (if in Ring 3): SS <- TSS.RSP0
                     RSP <- user RSP before interrupt
                     push SS, RSP, RFLAGS, CS, RIP
  |
  v
IRQ stub: push 0 (err_code), push 32 (int_no)
  |
  v
irq_common (interrupts.asm):
   push r15, r14, ..., rax  (15 GPRs)
   mov rdi, rsp
   call irq_handler          --> Rust: pit::tick(), keyboard dispatch
  |
  v  (send EOI to PIC)
   mov rdi, rsp
   call scheduler_tick       --> Rust: save current, pick next
   mov rsp, rax              --> SWITCH STACKS (may be same process)
  |
  v
   pop rax, rcx, ..., r15    (restore from new/current stack)
   add rsp, 16               (skip int_no + err_code)
   iretq                     (jump to user mode)
```

### 3.2 scheduler_tick -- Rust Function

```rust
/// Called from irq_common after EOI, interrupts disabled.
/// current_rsp = RSP pointing at saved RAX (the register frame).
/// Returns kernel_rsp of the next process to run.
#[no_mangle]
pub extern "C" fn scheduler_tick(current_rsp: u64) -> u64 {
    let cur_pid = current_pid();

    // 1. Save current kernel_rsp
    let cur = process_mut(cur_pid);
    cur.kernel_rsp = current_rsp;
    cur.state = ProcessState::Ready;  // was Running, now Ready

    // 2. Pick next ready process (round-robin)
    let next_pid = sched_next();

    // 3. Update globals
    set_current_pid(next_pid);

    // 4. Update TSS.RSP0 + per-process syscall kernel stack
    let next = process(next_pid);
    gdt::set_rsp0(next.kernel_stack_top);
    set_syscall_kstack(next.kernel_stack_top);

    // 5. Return next process's saved kernel_rsp
    next.kernel_rsp
}
```

### 3.3 sched_next -- Round-Robin

```rust
fn sched_next() -> u64 {
    let cur = current_pid();
    let table = unsafe { &PROCESS_TABLE };

    // Find current slot index
    let start = table.slots.iter().position(|s|
        s.as_ref().map_or(false, |p| p.pid == cur)
    ).unwrap_or(0);

    // Scan forward, wrap around
    for offset in 1..MAX_PROCS {
        let idx = (start + offset) % MAX_PROCS;
        if let Some(p) = &table.slots[idx] {
            if p.state == ProcessState::Ready {
                return p.pid;
            }
        }
    }
    cur  // no other ready process -- stay on current
}
```

### 3.4 Starting the First Process

```rust
/// Build synthetic register frame on init stack, switch to it. Never returns.
pub unsafe fn start_scheduler(init_pid: u64) -> ! {
    set_current_pid(init_pid);
    let proc = process(init_pid);
    set_syscall_kstack(proc.kernel_stack_top);
    gdt::set_rsp0(proc.kernel_stack_top);
    context_switch_to(proc.kernel_rsp)
}
```

```nasm
; context_switch_to(u64 kernel_rsp) -- assembly
global context_switch_to
context_switch_to:
    mov rsp, rdi
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
    add rsp, 16     ; skip int_no + err_code
    iretq
```

### 3.5 Building the Init Process Frame

```rust
pub fn build_init_frame(
    kernel_stack_top: u64,
    user_entry: u64,
    user_rsp: u64,
    command_id: u64,
) -> u64 {
    unsafe {
        let mut ptr = kernel_stack_top as *mut u64;

        // iretq frame (reverse order -- push last-to-first)
        ptr = ptr.sub(1); *ptr = 0x1Bu64;       // SS
        ptr = ptr.sub(1); *ptr = user_rsp;       // user RSP
        ptr = ptr.sub(1); *ptr = 0x202u64;       // RFLAGS
        ptr = ptr.sub(1); *ptr = 0x23u64;        // CS
        ptr = ptr.sub(1); *ptr = user_entry;     // RIP

        // int_no + err_code
        ptr = ptr.sub(1); *ptr = 0u64;           // err_code
        ptr = ptr.sub(1); *ptr = 0u64;           // int_no

        // GPRs (RAX first = lowest address)
        ptr = ptr.sub(1); *ptr = 0u64;           // RAX
        ptr = ptr.sub(1); *ptr = 0u64;           // RCX
        ptr = ptr.sub(1); *ptr = 0u64;           // RDX
        ptr = ptr.sub(1); *ptr = 0u64;           // RBX
        ptr = ptr.sub(1); *ptr = 0u64;           // RBP
        ptr = ptr.sub(1); *ptr = 0u64;           // RSI
        ptr = ptr.sub(1); *ptr = command_id;      // RDI (command selector!)
        ptr = ptr.sub(1); *ptr = 0u64;           // R8
        ptr = ptr.sub(1); *ptr = 0u64;           // R9
        ptr = ptr.sub(1); *ptr = 0u64;           // R10
        ptr = ptr.sub(1); *ptr = 0u64;           // R11
        ptr = ptr.sub(1); *ptr = 0u64;           // R12
        ptr = ptr.sub(1); *ptr = 0u64;           // R13
        ptr = ptr.sub(1); *ptr = 0u64;           // R14
        ptr = ptr.sub(1); *ptr = 0u64;           // R15

        ptr as u64  // = kernel_rsp (points at RAX)
    }
}
```

---

## 4. Syscall Changes

### 4.1 Per-Process Kernel Stack for Syscalls

**syscall_entry.asm** uses per-process stack instead of global:

```nasm
section .data
global current_proc_kernel_rsp
current_proc_kernel_rsp: dq 0      ; updated by scheduler

global syscall_state
syscall_state:                      ; SyscallSavedState layout
    .rsp:    dq 0
    .rflags: dq 0
    .rip:    dq 0

global should_schedule
should_schedule: db 0

section .text
global syscall_entry
syscall_entry:
    ; Save user RSP
    mov [rel syscall_state + 0], rsp      ; .rsp

    ; Switch to per-process kernel stack
    mov rsp, [rel current_proc_kernel_rsp]

    ; Save user RIP (RCX) + RFLAGS (R11)
    mov [rel syscall_state + 16], rcx     ; .rip
    mov [rel syscall_state + 8], r11      ; .rflags

    ; Setup C ABI: syscall_handler(num, arg1, arg2, arg3, arg4)
    mov r9, rdi        ; r9 = user arg1 (safe)
    mov rcx, rdx       ; rcx = arg3 = user rdx
    mov rdx, rsi       ; rdx = arg2 = user rsi
    mov rsi, r9        ; rsi = arg1 = user rdi
    mov rdi, rax       ; rdi = num

    call syscall_handler

    ; Check for pending reschedule (exit, waitpid block)
    cmp byte [rel should_schedule], 1
    je .exit_or_block

    ; Normal return via sysretq
    mov rcx, [rel syscall_state + 16]
    mov r11, [rel syscall_state + 8]
    mov rsp, [rel syscall_state + 0]
    db 0x48, 0x0f, 0x07    ; sysretq

.exit_or_block:
    mov byte [rel should_schedule], 0
    add rsp, 8             ; discard call return address

    ; Build full interrupt-compatible frame:
    push 0x1B                       ; SS
    push qword [rel syscall_state]  ; user RSP
    push 0x202                      ; RFLAGS
    push 0x23                       ; CS
    push qword [rel syscall_state + 16]  ; RIP
    push 0                          ; err_code
    push 0                          ; int_no
    push 0,0,0,0,0,0,0,0           ; RAX..RDI (padding)
    push 0,0,0,0,0,0,0,0           ; R8..R15

    ; RSP now points at RAX in the frame
    mov rdi, rsp
    extern scheduler_switch_exit
    call scheduler_switch_exit
    mov rsp, rax                   ; switch to next process

    pop rax, rcx, rdx, rbx, rbp, rsi, rdi
    pop r8, r9, r10, r11, r12, r13, r14, r15
    add rsp, 16
    iretq
```

### 4.2 scheduler_switch_exit

```rust
#[no_mangle]
pub extern "C" fn scheduler_switch_exit(current_rsp: u64) -> u64 {
    let cur_pid = current_pid();
    let cur = process_mut(cur_pid);
    cur.kernel_rsp = current_rsp;
    // state already set by handler (Zombie or Blocked)

    let next_pid = sched_next();
    set_current_pid(next_pid);

    let next = process(next_pid);
    gdt::set_rsp0(next.kernel_stack_top);
    set_syscall_kstack(next.kernel_stack_top);

    next.kernel_rsp
}
```

### 4.3 sys_exit

```rust
fn sys_exit(code: u64, _: u64, _: u64, _: u64) -> u64 {
    let pid = current_pid();
    serial_writeln!("VIBIX: PID {} exited with code {}", pid, code);

    let cur = process_mut(pid);
    cur.state = ProcessState::Zombie;
    cur.exit_code = code;

    // Wake parent if blocked on us
    let table = unsafe { &mut PROCESS_TABLE };
    for slot in &mut table.slots {
        if let Some(p) = slot {
            if p.state == ProcessState::Blocked && p.wait_for_pid == pid {
                p.state = ProcessState::Ready;
                p.wait_for_pid = 0;
            }
        }
    }

    unsafe { should_schedule = 1; }
    0  // value ignored; asm diverts to scheduler
}
```

### 4.4 sys_getpid

```rust
fn sys_getpid(_: u64, _: u64, _: u64, _: u64) -> u64 {
    current_pid()  // was hardcoded 1
}
```

### 4.5 sys_fork

```rust
fn sys_fork(_: u64, _: u64, _: u64, _: u64) -> u64 {
    let cur_pid = current_pid();
    let table = unsafe { &mut PROCESS_TABLE };

    // 1. Find free slot + allocate PID
    let child_slot = table.find_free_slot()?;
    let child_pid = table.alloc_pid();

    // 2. Allocate kernel stack for child
    let pmm = pmm::global_pmm();
    let kstack_page = pmm.alloc();
    if kstack_page.is_null() { return u64::MAX; }

    // 3. Copy parent's kernel stack contents
    let parent = process(cur_pid);
    let parent_kbase = parent.kernel_stack_top - KERNEL_STACK_SIZE as u64;
    let child_kbase = kstack_page as u64;
    unsafe {
        core::ptr::copy_nonoverlapping(
            parent_kbase as *const u8,
            child_kbase as *mut u8, KERNEL_STACK_SIZE);
    }

    // 4. Adjust child's saved RAX = 0 (fork returns 0 to child)
    let parent_offset = parent.kernel_stack_top - parent.kernel_rsp;
    let child_ktop = child_kbase + KERNEL_STACK_SIZE as u64;
    let child_krsp = child_ktop - parent_offset;
    unsafe { *(child_krsp as *mut u64) = 0; }

    // 5. Fill child descriptor
    let child = Process {
        pid: child_pid, state: ProcessState::Ready,
        entry: parent.entry, user_rsp: parent.user_rsp,
        kernel_stack_top: child_ktop, kernel_rsp: child_krsp,
        kernel_stack_base: child_kbase, parent_pid: cur_pid,
        exit_code: 0, wait_for_pid: 0,
        brk: parent.brk, errno: 0,
        name: *b"forked\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    };
    table.slots[child_slot] = Some(child);
    table.count += 1;
    child_pid
}
```

### 4.6 sys_exec

```rust
fn sys_exec(filename: u64, _argv: u64, _envp: u64) -> u64 {
    let pmm = pmm::global_pmm();
    let elf_data = unsafe {
        core::slice::from_raw_parts(filename as *const u8, 256 * 1024)
    };

    match elf::load(elf_data, pmm) {
        Ok(entry) => {
            let pid = current_pid();
            let proc = process_mut(pid);
            proc.entry = entry;
            proc.user_rsp = USER_STACK_ADDR + 0x1000;
            proc.brk = BRK_START;
            proc.errno = 0;

            // Redirect the in-flight syscall return
            unsafe {
                syscall_state.rip = entry;
                syscall_state.rsp = USER_STACK_ADDR + 0x1000;
                syscall_state.rflags = 0x202;
            }
            0
        }
        Err(_) => u64::MAX,
    }
}
```

### 4.7 sys_waitpid (Blocks with Yield)

```rust
fn sys_waitpid(pid: u64, status_ptr: u64, _options: u64) -> u64 {
    let cur = current_pid();
    let table = unsafe { &mut PROCESS_TABLE };

    // Find child
    let child_found = table.slots.iter().any(|s|
        s.as_ref().map_or(false, |p| p.pid == pid && p.parent_pid == cur));
    if !child_found { return u64::MAX; }  // ECHILD

    // Check if zombie
    for slot in &mut table.slots {
        if let Some(p) = slot {
            if p.pid == pid && p.state == ProcessState::Zombie {
                let ec = p.exit_code;
                if status_ptr != 0 {
                    unsafe { *(status_ptr as *mut u64) = ec; }
                }
                *slot = None;  // reap
                table.count -= 1;
                return pid;
            }
        }
    }

    // Child exists but not zombie -- block and yield
    let cur_proc = process_mut(cur);
    cur_proc.state = ProcessState::Blocked;
    cur_proc.wait_for_pid = pid;
    unsafe { should_schedule = 1; }
    0  // asm diverts to scheduler; when resumed, user retries
}
```

**Note**: Returns 0 to user space when the process is woken and rescheduled. User space must detect 0 and re-call waitpid in a loop. Future work: true blocking with automatic syscall restart.

### 4.8 Per-Process brk

```rust
fn sys_brk(addr: u64, _: u64, _: u64, _: u64) -> u64 {
    let pid = current_pid();
    let proc = process_mut(pid);

    if addr == 0 { return proc.brk; }  // sbrk(0)

    if addr < BRK_START || addr > BRK_MAX {
        proc.errno = ENOSYS;
        return u64::MAX;
    }

    let current_page_end = (proc.brk + 0xFFF) & !0xFFF;
    let new_page_end = (addr + 0xFFF) & !0xFFF;

    if new_page_end > current_page_end {
        let pmm = pmm::global_pmm();
        let mut vaddr = current_page_end;
        while vaddr < new_page_end {
            let phys = pmm.alloc();
            if phys.is_null() {
                proc.errno = ENOSYS;
                return u64::MAX;
            }
            unsafe { core::ptr::write_bytes(phys, 0, 4096); }
            paging::map_4k(vaddr, phys as u64, paging::PAGE_USER_RW, pmm);
            vaddr += 4096;
        }
    }
    proc.brk = addr;
    addr
}
```

### 4.9 Syscall Number Table

```rust
pub fn init() {
    register(0, sys_exit);
    register(1, sys_write);     // unchanged
    register(2, sys_read);      // unchanged
    register(3, sys_getpid);    // now uses current_pid()
    register(4, sys_brk);       // now per-process
    register(5, sys_fork);      // NEW
    register(6, sys_exec);      // NEW
    register(7, sys_waitpid);   // NEW
}
```

---

## 5. lib.rs Boot Flow

```rust
pub extern "C" fn kernel_main() -> ! {
    // ... existing: serial, pmm, paging, fb, interrupts, pit, gdt,
    //     syscall init, enable_interrupts() ...

    // Create init process
    let init_pid = process::spawn_init(&mut pmm);
    serial.writestrs(&["VIBIX: Created PID 1 (init).\n"]);

    // Start the scheduler -- never returns
    serial.writestrs(&["VIBIX: Starting scheduler...\n"]);
    unsafe { process::start_scheduler(init_pid); }
}
```

### spawn_init helper

```rust
pub fn spawn_init(pmm: &mut PmmAllocator) -> u64 {
    // Load binary into user space (same as current create_init)
    load_init_binary(pmm);  // maps USER_CODE_ADDR and USER_STACK_ADDR

    // Allocate kernel stack for init
    let page = pmm.alloc();
    let ktop = page as u64 + KERNEL_STACK_SIZE as u64;

    // Build register frame (command_id = 1 for init_demo)
    let krsp = build_init_frame(ktop, USER_CODE_ADDR,
                                USER_STACK_ADDR + 0x1000, 1);

    let table = unsafe { &mut PROCESS_TABLE };
    table.slots[0] = Some(Process {
        pid: 1, state: ProcessState::Ready,
        entry: USER_CODE_ADDR,
        user_rsp: USER_STACK_ADDR + 0x1000,
        kernel_stack_top: ktop, kernel_rsp: krsp,
        kernel_stack_base: page as u64,
        parent_pid: 0, exit_code: 0, wait_for_pid: 0,
        brk: BRK_START, errno: 0,
        name: *b"init\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    });
    table.count = 1;
    table.next_pid = 2;
    1
}
```

---

## 6. GDT/TSS Change

Add one function to gdt.rs:

```rust
pub unsafe fn set_rsp0(rsp0: u64) {
    TSS.rsp0 = rsp0;
}
```

---

## 7. File-by-File Change Plan

### New Files

| File | Contents |
|------|----------|
| kernel/context_switch.asm | context_switch_to(), force_schedule() |

### Modified Files

| File | Changes |
|------|---------|
| kernel_rust/src/process.rs | FULL REWRITE: Process, ProcessState, ProcessTable, spawn_init(), build_init_frame(), start_scheduler(), sched_next(), scheduler_tick(), scheduler_switch_exit(), set_syscall_kstack(), current_pid(), process_mut(), load_init_binary(). |
| kernel_rust/src/syscall.rs | Modify sys_exit, sys_getpid, sys_brk. Add sys_fork, sys_exec, sys_waitpid. Remove global PROGRAM_BREAK, ERRNO. Update init(). |
| kernel_rust/src/gdt.rs | Add pub unsafe fn set_rsp0(u64). |
| kernel_rust/src/lib.rs | Replace create_init+enter_user_mode with spawn_init+start_scheduler. |
| kernel/syscall_entry.asm | Add current_proc_kernel_rsp, syscall_state, should_schedule globals. Use per-process stack. Add exit/block scheduler path. |
| kernel/interrupts.asm | Modify irq_common: call scheduler_tick after EOI, switch stacks. |
| Makefile | Add context_switch.o to link line. |

### Unchanged

| File | Reason |
|------|--------|
| kernel_rust/src/pit.rs | tick() unchanged; context switch happens in asm after dispatch |
| kernel_rust/src/paging.rs | No changes needed |
| kernel_rust/src/pmm.rs | No changes needed |
| kernel_rust/src/elf.rs | Already correct |
| kernel_rust/src/interrupts.rs | No changes needed (handler dispatched by asm) |

---

## 8. Edge Cases

### All processes blocked
If all are Blocked, sched_next() returns current PID. Timer ticks spin. Future: idle process with HLT.

### Init exits
Marked Zombie, no parent to wake. Last process gone -> kernel should HLT. Short-term: check if all slots are Zombie/None after exit, HLT.

### Fork with full table
Return u64::MAX. Userspace checks and handles.

### Kernel stack overflow
4 KiB per process. Frame ~176 bytes + C frames ~256 bytes = ~432 bytes. 4 KiB is safe but tight. Bump to 8192 if debugging shows corruption.

### Interrupts during scheduler
CPU clears IF on interrupt gate entry (our IDT setup). scheduler_tick runs with IF=0. iretq restores IF from saved RFLAGS.

---

## 9. Future Work

1. ~~Idle process (HLT loop) for power saving~~ ✅ DONE (PID 2)
2. ~~IPC (pipes)~~ ✅ DONE (syscall 20)
3. Signals + process groups — needed for Ctrl+C / job control (see [phase3c.md](phase3c.md))
4. True blocking waitpid with automatic syscall restart
5. Address space isolation (per-process page tables)
6. COW fork
7. Shared memory (shm)
8. Kernel stack reclamation on process exit
9. Syscall times / per-process accounting
10. Per-CPU data for SMP
