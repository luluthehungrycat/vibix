## Why

The current ELF loader stores load-local page provenance in a fixed 256-entry array. Valid executables spanning more than 256 unique pages are rejected as `ElfError::Oom`, and the rejection can occur after the new image has been partially mapped.

## What Changes

- Replace the fixed provenance ceiling with scalable metadata appropriate for bounded kernel execution.
- Define accurate allocation and metadata exhaustion error behavior.
- Define cleanup or rollback expectations for every load failure after mappings or frames are created.
- Preserve shared PT_LOAD page reuse, old-image isolation, and ordered partial-page copying.
- Add ELF fixtures and failure tests beyond 256 pages.

## Capabilities

### New Capabilities
- `scalable-elf-page-provenance`: Tracks all pages owned by one ELF load without an arbitrary 256-page limit and reports recoverable load failures safely.

### Modified Capabilities

<!-- No existing capability specifications are present in the repository-local OpenSpec tree. -->

## Impact

- Affects the VIBIX ELF loader, process exec failure handling, PMM/page-table ownership, and focused Rust ELF tests.
- No page-table architecture rewrite, external repository change, or permission/NX policy change is in scope unless required by the failure-semantics design.
