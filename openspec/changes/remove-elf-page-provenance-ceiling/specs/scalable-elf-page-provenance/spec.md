## ADDED Requirements

### Requirement: ELF provenance scales with the loaded image
The ELF loader SHALL track every page allocated by one load without a fixed 256-page ceiling, subject only to actual metadata and physical-memory availability.

#### Scenario: Image exceeds 256 unique pages
- **WHEN** a valid ELF requires more than 256 unique pages
- **THEN** loading continues until completion or a real resource limit

#### Scenario: Overlapping PT_LOAD pages are shared
- **WHEN** a later PT_LOAD covers a page recorded by this load
- **THEN** that frame is reused and later partial-page bytes are preserved

### Requirement: ELF failures report real resource exhaustion
The loader SHALL distinguish malformed/truncated input, physical-memory exhaustion, and provenance-metadata exhaustion.

#### Scenario: Physical page allocation fails
- **WHEN** PMM cannot provide a page
- **THEN** loading returns physical-memory exhaustion, not a fixed page-count error

#### Scenario: Metadata allocation fails
- **WHEN** provenance storage cannot grow
- **THEN** loading returns metadata-resource exhaustion and rolls back

### Requirement: Failed loads leave no loader-owned partial image
On error after mapping begins, the loader SHALL remove mappings and release frames allocated by this invocation while preserving inherited mappings.

#### Scenario: Failure after partial mapping
- **WHEN** a later segment is truncated or allocation fails
- **THEN** all tracked pages are unmapped/released, metadata is freed, and unrelated mappings remain

#### Scenario: Successful load transfers ownership
- **WHEN** all PT_LOAD data copies successfully
- **THEN** temporary metadata is freed and loaded mappings remain owned by the new image
