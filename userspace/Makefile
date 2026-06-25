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

# Sources
BLOB_ASM      = $(BUILD_DIR)/vibix_blob.asm

VIBIX_INC     = $(BUILD_DIR)/vibix_core.inc   \
                $(BUILD_DIR)/vibix_tiny.inc   \
                $(BUILD_DIR)/vibix_echo.inc   \
                $(BUILD_DIR)/vibix_cat.inc    \
                $(BUILD_DIR)/vibix_printenv.inc \
                $(BUILD_DIR)/vibix_clear.inc  \
                $(BUILD_DIR)/vibix_shell.inc

OUTPUT        = $(BUILD_DIR)/vibix_blob.bin
INITRAMFS     = $(BUILD_DIR)/initramfs.tar

.PHONY: all clean

all: $(OUTPUT) $(INITRAMFS)

$(OUTPUT): $(BLOB_ASM) $(VIBIX_INC)
	$(NASM) -f bin -I $(INC_DIR)/ $< -o $@

$(INITRAMFS): $(OUTPUT)
	mkdir -p /tmp/vibix-initramfs/sbin
	cp $(OUTPUT) /tmp/vibix-initramfs/sbin/init
	cd /tmp/vibix-initramfs && tar --format=ustar --owner=0 --group=0 -cf $(CURDIR)/$@ sbin/init
	rm -rf /tmp/vibix-initramfs

clean:
	rm -f $(OUTPUT) $(INITRAMFS)
