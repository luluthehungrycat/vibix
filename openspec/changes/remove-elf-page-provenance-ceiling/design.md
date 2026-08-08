## Context

`elf::load` uses a stack-resident `[Option<LoadedPage>; 256]`. This correctly keeps provenance load-local but rejects valid images with more than 256 unique pages and has no explicit rollback contract after partial mapping. The kernel is `no_std` but provides `kmm::kmalloc/kfree`; PMM and target-page-table ownership must remain isolated.

## Goals / Non-Goals

**Goals:** remove the arbitrary ceiling, preserve same-load overlap reuse, distinguish real resource errors, and roll back every page owned by a failed load.

**Non-Goals:** the historical paging rewrite, permission/NX changes, or reuse of inherited mappings.

## Decisions

- Use growable load-local provenance storage backed by kernel heap chunks, not a larger fixed stack array.
- Record each new page immediately and search only this tracker for overlap reuse.
- Add distinct metadata-resource failure behavior while retaining PMM exhaustion and malformed-input errors.
- Make failure transactional: unmap/release only pages recorded by this invocation, then free metadata; successful loads transfer page ownership and free temporary metadata.

## Risks / Trade-offs

- [Metadata growth fails after mapping] → grow before committing a page where possible and always run rollback.
- [Cleanup disturbs target CR3/TLB] → use the existing interrupt/CR3 discipline and required invalidation.
- [Inherited mapping is deleted] → cleanup may operate only on tracker entries.

## Migration Plan

No on-disk migration. Implement loader/test changes, run ELF, kernel, and VIBIT suites. Reverting the VIBIX loader/test changes is the rollback.

## Open Questions

Which existing paging/PMM helper should be canonical for target-PML4 unmap-and-free cleanup?
