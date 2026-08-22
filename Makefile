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
RUST_TOOLCHAIN ?= stable
CARGO       = cargo +$(RUST_TOOLCHAIN)
OBJCOPY     = objcopy

# 64-bit kernel build (Rust staticlib + asm entry)
RUST_TARGET = x86_64-unknown-none
RUST_DIR    = kernel_rust
RUST_LIB    = $(RUST_DIR)/target/$(RUST_TARGET)/release/libvibix_kernel.a
RUST_LD     = $(RUST_DIR)/kernel64_elf.ld

# Debug flag: make DEBUG=1 to enable kernel debug output
DEBUG ?= 0
RUST_FEATURES = $(if $(filter 1,$(DEBUG)),--features debug,)
NASMFLAGS = $(if $(filter 1,$(DEBUG)),-d DEBUG,)

# Test-only escape hatch: a harness may stage a temporary initramfs archive and
# then rebuild the kernel without userspace/Makefile regenerating it.
VIBIX_STAGED_INITRAMFS ?= 0

# Init selection: make INIT=vibit to use VIBIT init system from ../vibit
INIT ?= default

# Final 32-bit ELF flags (Multiboot wrapper)
ASMFLAGS32  = -f elf32
LDFLAGS32   = -m elf_i386 -T linker.ld -nostdlib

.PHONY: all clean run debug test_tty_sigint test_elf_rollback

all: vibix.elf

# ── Combined userspace binary (delegated to userspace/Makefile) ──────────────

USR_BIN = userspace/vibix_blob.bin
USR_TAR = userspace/initramfs.tar

ifeq ($(VIBIX_STAGED_INITRAMFS),1)
$(USR_BIN) $(USR_TAR):
	@test -s "$(USR_TAR)" || { echo "ERROR: staged initramfs is missing or empty" >&2; exit 1; }
else
$(USR_BIN) $(USR_TAR):
	$(MAKE) -C userspace all INIT=$(INIT)
endif

# ── Stage 1: 64-bit flat binary ─────────────────────────────────────────────

kernel64_entry.o: kernel/kernel64_entry.asm
	$(NASM) -f elf64 $(NASMFLAGS) $< -o $@

interrupts.o: kernel/interrupts.asm
	$(NASM) -f elf64 $(NASMFLAGS) $< -o $@

syscall_entry.o: kernel/syscall_entry.asm
	$(NASM) -f elf64 $(NASMFLAGS) $< -o $@

context_switch.o: kernel/context_switch.asm
	$(NASM) -f elf64 $(NASMFLAGS) $< -o $@

# Build the Rust staticlib (produces libvibix_kernel.a) with the repository's
# explicitly selected stable toolchain.  CI installs the same toolchain before
# invoking this root Makefile.
# Use `make DEBUG=1` to enable kernel debug output
$(RUST_LIB): $(USR_BIN) $(USR_TAR) $(wildcard $(RUST_DIR)/src/*.rs) $(RUST_DIR)/Cargo.toml
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
	$(QEMU) $(QEMU_FLAGS) -L /usr/share/qemu -serial stdio -display none

debug: $(USR_BIN) vibix.elf
	$(QEMU) $(QEMU_FLAGS) -L /usr/share/qemu -serial stdio -display none -s -S

test: vibix.elf
	python3 anti_cheat.py && python3 test_kernel.py

test_vibit: clean
	$(MAKE) INIT=vibit
	python3 anti_cheat.py && python3 -c "from test_kernel import test_vibit; import sys; sys.exit(0 if test_vibit() else 1)"

# Run the deterministic DEBUG-only TTY ownership fixture.
test_tty_sigint: clean
	$(MAKE) DEBUG=1
	python3 anti_cheat.py && python3 -c "from test_kernel import test_tty_sigint; import sys; sys.exit(0 if test_tty_sigint() else 1)"

# Run the deterministic DEBUG-only ELF partial-load rollback fixture.
test_elf_rollback: clean
	$(MAKE) DEBUG=1
	python3 anti_cheat.py && python3 -c "from test_kernel import test_elf_rollback; import sys; sys.exit(0 if test_elf_rollback() else 1)"

clean:
	rm -f *.o *.elf *.bin kernel/interrupts.o kernel/syscall_entry.o kernel/context_switch.o
	$(MAKE) -C userspace clean
	cd $(RUST_DIR) && $(CARGO) clean 2>/dev/null || true

test_vibit_rust: clean
	# Build the sibling Rust ELF before staging it, even when no artifact exists yet.
	@if [ ! -d "$(CURDIR)/../vish" ]; then \
		echo "ERROR: Rust vish prerequisite is missing: expected sibling checkout at $(CURDIR)/../vish" >&2; \
		echo "       Provide ../vish with its documented 'elf' target, then rerun: make test_vibit_rust" >&2; \
		exit 1; \
	fi
	@if ! $(MAKE) -C "$(CURDIR)/../vish" elf; then \
		echo "ERROR: Rust vish prerequisite failed: '$(MAKE) -C ../vish elf'" >&2; \
		echo "       Install the sibling vish Rust toolchain/dependencies and rerun: make test_vibit_rust" >&2; \
		exit 1; \
	fi
	@if [ ! -s "$(CURDIR)/../vish/target/x86_64-unknown-none/release/vibix" ]; then \
		echo "ERROR: Rust vish prerequisite produced no non-empty ELF at $(CURDIR)/../vish/target/x86_64-unknown-none/release/vibix" >&2; \
		echo "       Ensure the sibling 'elf' target produces that path, then rerun: make test_vibit_rust" >&2; \
		exit 1; \
	fi
	# Build the kernel Rust library with DEBUG=1 so the loader diagnostics are present.
	$(MAKE) DEBUG=1 INIT=vibit
	# Replace /bin/vish with Rust ELF to test multi-segment ELF scheduling
	rm -rf /tmp/vibix-initramfs
	mkdir -p /tmp/vibix-initramfs/sbin /tmp/vibix-initramfs/bin
	tar xf $(CURDIR)/userspace/initramfs.tar -C /tmp/vibix-initramfs
	cp $(CURDIR)/../vish/target/x86_64-unknown-none/release/vibix /tmp/vibix-initramfs/bin/vish
	cd /tmp/vibix-initramfs && tar --format=ustar --owner=0 --group=0 -cf $(CURDIR)/userspace/initramfs.tar sbin/init bin/*
	rm -rf /tmp/vibix-initramfs
	# Force only the DEBUG assembly objects; do not rebuild userspace after
	# replacing /bin/vish with the Rust ELF above.
	rm -f kernel64_entry.o interrupts.o syscall_entry.o context_switch.o
	$(MAKE) DEBUG=1 kernel64_entry.o interrupts.o syscall_entry.o context_switch.o
	$(MAKE) DEBUG=1 kernel64.elf kernel64.bin boot.o vibix.elf
	python3 anti_cheat.py && python3 -c "from test_kernel import test_vibit_rust; import sys; sys.exit(0 if test_vibit_rust() else 1)"

# Run the VIBIX-owned synthetic ELF that requires more than 256 pages through
# VIBIT's real fork/exec("/bin/vish") path.  The Python harness temporarily
# replaces userspace/initramfs.tar, then uses VIBIX_STAGED_INITRAMFS=1 while
# rebuilding so the normal userspace staging recipe cannot overwrite it.
test_vibit_rust_large: clean
	$(MAKE) INIT=vibit userspace/initramfs.tar
	python3 anti_cheat.py && python3 -c "from test_kernel import test_vibit_rust_large; import sys; sys.exit(0 if test_vibit_rust_large() else 1)"
