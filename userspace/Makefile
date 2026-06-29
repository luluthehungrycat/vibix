#==============================================================================
# Makefile for VIBIX userspace binaries
#
# Builds the combined flat binary (vibix_blob.bin) that the kernel embeds
# at link time.  Can be built independently or invoked from the root Makefile.
#
# Output: userspace/vibix_blob.bin
#         (referenced by kernel_rust/src/process.rs via include_bytes!)
#==============================================================================

NASM          = nasm
BUILD_DIR     = .
INC_DIR       = $(BUILD_DIR)

# Init source selection:
#   make INIT=vibit   → use VIBIT init system from ../vibit
#   make (default)    → use vibix_blob.asm (test/development blob)
INIT ?= default

OUTPUT        = $(BUILD_DIR)/vibix_blob.bin
INITRAMFS     = $(BUILD_DIR)/initramfs.tar

# ── Default init: vibix_blob.asm (test/development combined blob) ─────────────
BLOB_ASM      = $(BUILD_DIR)/vibix_blob.asm
VIBIX_INC     = $(BUILD_DIR)/vibix_core.inc   \
                $(BUILD_DIR)/vibix_tiny.inc   \
                $(BUILD_DIR)/vibix_echo.inc   \
                $(BUILD_DIR)/vibix_cat.inc    \
                $(BUILD_DIR)/vibix_printenv.inc \
                $(BUILD_DIR)/vibix_clear.inc  \
                $(BUILD_DIR)/vibix_shell.inc

# ── VIBIT init: use the external VIBIT repo's binary if INIT=vibit ──────────
VIBIT_DIR     = $(abspath $(BUILD_DIR)/../../vibit)
VIBIT_BIN     = $(VIBIT_DIR)/vibit.bin
VISH_DIR      = $(abspath $(BUILD_DIR)/../../vish)
VISH_BIN      = $(VISH_DIR)/vish.bin

.PHONY: all clean

all: $(OUTPUT) $(INITRAMFS)

# Build or copy the init binary
$(OUTPUT):
ifeq ($(INIT),vibit)
	$(MAKE) -C $(VIBIT_DIR) all
	cp $(VIBIT_BIN) $@
else
	$(NASM) -f bin -I $(INC_DIR)/ $(BLOB_ASM) -o $@
endif

$(INITRAMFS): $(OUTPUT)
	mkdir -p /tmp/vibix-initramfs/sbin /tmp/vibix-initramfs/bin
	cp $(OUTPUT) /tmp/vibix-initramfs/sbin/init
ifeq ($(INIT),vibit)
	# Build and copy vish shell binary for VIBIT to exec
	$(MAKE) -C $(VISH_DIR) all
	cp $(VISH_BIN) /tmp/vibix-initramfs/bin/vish
endif
	cd /tmp/vibix-initramfs && tar --format=ustar --owner=0 --group=0 -cf $(CURDIR)/$@ sbin/init bin/vish 2>/dev/null || tar --format=ustar --owner=0 --group=0 -cf $(CURDIR)/$@ sbin/init
	rm -rf /tmp/vibix-initramfs

clean:
	rm -f $(OUTPUT) $(INITRAMFS)
