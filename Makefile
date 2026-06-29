#==============================================================================
# Makefile for VIBIX — two-stage build
#
# Stage 1:  64-bit Rust code + asm entry → flat binary (kernel64.bin) at
#           0x200000 via cargo + ld + objcopy.
# Stage 2:  boot.asm (ELF32, Multiboot v1) embeds kernel64.bin via incbin.
# Final:    vibix.elf — 32-bit ELF accepted by QEMU -kernel.
#==============================================================================

NASM        = nasm
LD          = ld
CARGO       = cargo
OBJCOPY     = objcopy

# 64-bit kernel build (Rust staticlib + asm entry)
RUST_TARGET = x86_64-unknown-none
RUST_DIR    = kernel_rust
RUST_LIB    = $(RUST_DIR)/target/$(RUST_TARGET)/release/libvibix_kernel.a
RUST_LD     = $(RUST_DIR)/kernel64_elf.ld

# Debug flag: make DEBUG=1 to enable kernel debug output
DEBUG ?= 0
RUST_FEATURES = $(if $(filter 1,$(DEBUG)),--features debug,)

# Init selection: make INIT=vibit to use VIBIT init system from ../vibit
INIT ?= default

# Final 32-bit ELF flags (Multiboot wrapper)
ASMFLAGS32  = -f elf32
LDFLAGS32   = -m elf_i386 -T linker.ld -nostdlib

.PHONY: all clean run debug

all: vibix.elf

# ── Combined userspace binary (delegated to userspace/Makefile) ──────────────

USR_BIN = userspace/vibix_blob.bin
USR_TAR = userspace/initramfs.tar

$(USR_BIN) $(USR_TAR):
	$(MAKE) -C userspace all INIT=$(INIT)

# ── Stage 1: 64-bit flat binary ─────────────────────────────────────────────

kernel64_entry.o: kernel/kernel64_entry.asm
	$(NASM) -f elf64 $< -o $@

interrupts.o: kernel/interrupts.asm
	$(NASM) -f elf64 $< -o $@

syscall_entry.o: kernel/syscall_entry.asm
	$(NASM) -f elf64 $< -o $@

context_switch.o: kernel/context_switch.asm
	$(NASM) -f elf64 $< -o $@

# Build the Rust staticlib (produces libvibix_kernel.a)
# Use `make DEBUG=1` to enable kernel debug output
$(RUST_LIB): $(USR_BIN) $(wildcard $(RUST_DIR)/src/*.rs) $(RUST_DIR)/Cargo.toml
	cd $(RUST_DIR) && \
	RUSTFLAGS="-C code-model=kernel" \
	$(CARGO) build --target $(RUST_TARGET) --release $(RUST_FEATURES)

# Link asm entry + interrupt stubs + Rust staticlib into an ELF
kernel64.elf: kernel64_entry.o interrupts.o syscall_entry.o context_switch.o $(RUST_LIB)
	$(LD) -T $(RUST_LD) -nostdlib -o $@ kernel64_entry.o interrupts.o syscall_entry.o context_switch.o $(RUST_LIB)

# Flatten to flat binary for incbin
kernel64.bin: kernel64.elf
	$(OBJCOPY) -O binary $< $@

# ── Stage 2: 32-bit ELF wrapper ─────────────────────────────────────────────

boot.o: boot.asm kernel64.bin
	$(NASM) $(ASMFLAGS32) $< -o $@

vibix.elf: boot.o
	$(LD) $(LDFLAGS32) -o $@ $<

# ── Convenience targets ──────────────────────────────────────────────────────

QEMU        = /usr/bin/qemu-system-x86_64
QEMU_FLAGS  = -accel kvm -kernel vibix.elf -m 512M -no-reboot -no-shutdown

run: $(USR_BIN) vibix.elf
	$(QEMU) $(QEMU_FLAGS) -serial stdio -display none

debug: $(USR_BIN) vibix.elf
	$(QEMU) $(QEMU_FLAGS) -serial stdio -display none -s -S

test: vibix.elf
	python3 test_kernel.py

clean:
	rm -f *.o *.elf *.bin kernel/interrupts.o kernel/syscall_entry.o kernel/context_switch.o
	$(MAKE) -C userspace clean
	cd $(RUST_DIR) && $(CARGO) clean 2>/dev/null || true
