# Phase 44 Review: Homebrew Loader v0

Date: 2026-06-26
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-import/src/lib.rs` | `.nxhb.toml` parser, metadata model, code/data/stack mapping, range validation |
| `nx86-vmm/src/lib.rs` | public page permission transition for write-seed then read/execute code pages |
| `nx86-hle/src/lib.rs` | minimal service table: `svc #0` clean exit, unknown SVC reporting |
| `nx86-runtime/src/lib.rs` | `boot_homebrew` helper that maps a module, seeds PC/SP, interprets, and classifies SVC outcome |
| `nx86-title-db/src/lib.rs` | `homebrew` source kind and content persistence for module TOML |

## Findings

### FINDING-1: importer crate was still a stub (fixed)

`nx86-import` now owns a deterministic homebrew descriptor format with explicit
metadata, code, entrypoint, stack, optional segment, and validation rules. It
rejects malformed hex, unaligned program length, invalid permissions, overlapping
ranges, and entrypoints outside the loaded program.

### FINDING-2: loaded code needed a permission transition (fixed)

The loader must write code bytes before exposing the page as executable. VMM now
supports changing page permissions after mapping, so code can be seeded through a
writable mapping and then reprotected as read/execute.

### FINDING-3: SVC handling needed a narrow Phase 44 boundary (fixed)

The HLE surface only recognizes `svc #0` as homebrew exit with x0 as the status
code. Other SVCs are reported as unhandled so Phase 45 can add service dispatch
without pretending broader HLE already exists.

## Test coverage

| Test | What |
|------|------|
| `parses_homebrew_metadata_program_and_segments` | descriptor metadata, program bytes, segment permissions |
| `maps_program_data_and_stack_into_guest_memory` | code/data/stack mapping and final permissions |
| `simple_homebrew_boots_to_clean_exit` | runtime entrypoint/SP seeding and `svc #0` exit classification |
| `homebrew_unknown_svc_is_reported` | unknown service calls remain visible |
| `homebrew_title_persists_and_reads_back_content` | title-db source kind and content persistence |
| `page_permissions_can_be_changed_after_loading` | VMM permission transition after loader writes |

## Verification

```
cargo test -p nx86-import -p nx86-hle -p nx86-runtime -p nx86-title-db -p nx86-vmm --lib -> PASS
just verify -> PASS
```
