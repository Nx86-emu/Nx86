# Phase 42 Review: Hot/Cold Splitting

Date: 2026-06-25
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-x64-v4/src/lib.rs` | `CodeSection`, `BlockLayoutEntry`, `HotColdLayout`, `hot_cold_layout`, `lower_function_with_profile` |
| `crates/nx86-x64-v4/Cargo.toml` | dependency on `nx86-profile` for profile-driven layout input |

## Findings

### FINDING-1: native block order ignored profile heat (fixed)

`lower_function` preserved source block order only. The new profiled lowering
path keeps entry first, follows the hottest observed successor chain, then moves
unobserved blocks to a cold tail.

### FINDING-2: native mapping must survive reordering (verified)

The profiled lowerer still resolves branch targets through the full function
entry table and returns each `LoweredFunctionBlock` keyed by guest entry PC.
Dispatcher identity is therefore independent of emitted list order.

## Test coverage

| Test | What |
|------|------|
| `profiled_layout_moves_hot_successor_before_cold_block` | hot successor moves ahead of cold original-order block and branch mapping still targets the correct PC |

## Verification

```
cargo test -p nx86-x64-v4 --lib profiled_layout_moves_hot_successor_before_cold_block -> PASS
```
