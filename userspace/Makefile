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
VISH_RUST_ELF = $(VISH_DIR)/target/x86_64-unknown-none/release/vibix
GVIBU_DIR     = $(abspath $(BUILD_DIR)/../../gvibu-ai-lab/kernel)
GVIBU_ELF_DIR = $(abspath $(BUILD_DIR)/../../gvibu-ai-lab/vibix-lib)

.PHONY: all clean FORCE
.ONESHELL:

FORCE:

ifeq ($(INIT),vibit)
$(OUTPUT) $(INITRAMFS): FORCE
endif

all: $(OUTPUT) $(INITRAMFS)

# Build or copy the init binary
$(OUTPUT):
ifeq ($(INIT),vibit)
	@test -d "$(VIBIT_DIR)" || { printf '%s\n' "ERROR: missing ../vibit; run make -C ../vibit all from the VIBIX root." >&2; exit 1; }
	$(MAKE) -C "$(VIBIT_DIR)" all || { printf '%s\n' "ERROR: VIBIT build failed in ../vibit; run make -C ../vibit all." >&2; exit 1; }
	@test -s "$(VIBIT_BIN)" || { printf '%s\n' "ERROR: missing or empty ../vibit/vibit.bin after make -C ../vibit all." >&2; exit 1; }
	cp -f "$(VIBIT_BIN)" "$@"
else
	$(NASM) -f bin -I $(INC_DIR)/ $(BLOB_ASM) -o $@
endif

$(INITRAMFS): $(OUTPUT)
	tmp=$$(mktemp -d "$${TMPDIR:-/tmp}/vibix-initramfs.XXXXXX") || { printf '%s\n' "ERROR: unable to create temporary initramfs directory." >&2; exit 1; }; \
	trap 'rm -rf "$$tmp"' EXIT HUP INT TERM; \
	mkdir -p "$$tmp/sbin" "$$tmp/bin" || { printf '%s\n' "ERROR: unable to prepare temporary initramfs tree." >&2; exit 1; }; \
	cp -f "$(OUTPUT)" "$$tmp/sbin/init" || { printf '%s\n' "ERROR: unable to stage /sbin/init from $(OUTPUT)." >&2; exit 1; };
ifeq ($(INIT),vibit)
	$(MAKE) -C "$(VISH_DIR)" nasm || { printf '%s\n' "ERROR: NASM vish build failed in ../vish; run make -C ../vish nasm." >&2; exit 1; }
	test -s "$(VISH_BIN)" || { printf '%s\n' "ERROR: missing or empty ../vish/vish.bin after make -C ../vish nasm." >&2; exit 1; }
	cp -f "$(VISH_BIN)" "$$tmp/bin/vish" || { printf '%s\n' "ERROR: unable to stage /bin/vish from ../vish/vish.bin." >&2; exit 1; }
endif
ifeq ($(INIT),vibit)
	cd "$$tmp" && tar --format=ustar --owner=0 --group=0 -cf "$(CURDIR)/$@" sbin/init bin/vish || { printf '%s\n' "ERROR: unable to create initramfs archive $(CURDIR)/$@." >&2; exit 1; }
else
	cd "$$tmp" && tar --format=ustar --owner=0 --group=0 -cf "$(CURDIR)/$@" sbin/init || { printf '%s\n' "ERROR: unable to create initramfs archive $(CURDIR)/$@." >&2; exit 1; }
endif

clean:
	rm -f $(OUTPUT) $(INITRAMFS)
