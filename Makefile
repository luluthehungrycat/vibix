#==============================================================================
# Makefile for VIBIX — two-stage build
#
# Stage 1:  64-bit C code → flat binary (kernel64.bin) at 0x200000.
# Stage 2:  boot.asm (ELF32, Multiboot v1) embeds kernel64.bin via incbin.
# Final:    vibix.elf — 32-bit ELF accepted by QEMU -kernel.
#==============================================================================

NASM       = nasm
CC         = gcc
LD         = ld

# 64-bit kernel flags
CFLAGS64   = -ffreestanding -nostdlib -m64 -mno-red-zone -Iinclude \
             -Wall -Wextra -Werror
LDFLAGS64  = -T kernel64.ld -nostdlib

# Final 32-bit ELF flags (Multiboot wrapper)
ASMFLAGS32 = -f elf32
LDFLAGS32  = -m elf_i386 -T linker.ld -nostdlib

C_OBJS64   = kernel.o serial.o pmm.o
ASM_OBJS64 = kernel64_entry.o

.PHONY: all clean run debug

all: vibix.elf

# ── Stage 1: 64-bit flat binary ─────────────────────────────────────────────

$(C_OBJS64): %.o: kernel/%.c
	$(CC) $(CFLAGS64) -c $< -o $@

kernel64_entry.o: kernel/kernel64_entry.asm
	$(NASM) -f elf64 $< -o $@

kernel64.bin: kernel64_entry.o $(C_OBJS64)
	$(LD) $(LDFLAGS64) -o $@ $^

# ── Stage 2: 32-bit ELF wrapper ─────────────────────────────────────────────

boot.o: boot.asm kernel64.bin
	$(NASM) $(ASMFLAGS32) $< -o $@

vibix.elf: boot.o
	$(LD) $(LDFLAGS32) -o $@ $<

# ── Convenience targets ──────────────────────────────────────────────────────

QEMU        = /usr/bin/qemu-system-x86_64
QEMU_FLAGS  = -accel tcg -kernel vibix.elf -m 512M -no-reboot -no-shutdown

run: vibix.elf
	$(QEMU) $(QEMU_FLAGS) -serial stdio -display none

debug: vibix.elf
	$(QEMU) $(QEMU_FLAGS) -serial stdio -display none -s -S

test: vibix.elf
	python3 test_kernel.py

clean:
	rm -f *.o *.elf *.bin
