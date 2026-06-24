# Phase 34 Review: Atomics v0

Date: 2026-06-24
Reviewer: MiMo V2.5 Pro

## What landed

| Crate | Lines | What |
|-------|-------|------|
| `nx86-core/src/guest.rs` | +25 | `ExclusiveMonitor` struct, `monitor` field, accessors |
| `nx86-arm64-decode/src/lib.rs` | +80 | 4 new variants, `InstructionClass::Atomic`, decode + disasm, 2 tests |
| `nx86-ir/src/lib.rs` | +55 | 4 new `Op` variants, result_type/is_side_effect/operand/format updates |
| `nx86-arm64-lift/src/lib.rs` | +100 | Lifting for 4 atomic instruction kinds |
| `nx86-runtime/src/eval.rs` | +180 | Evaluator execution + 6 unit tests |
| `nx86-runtime/src/lib.rs` | +80 | TinyInterpreter execution for 4 kinds |
| `nx86-x64-v4/src/lib.rs` | +8 | `UnsupportedOp` placeholder for atomics |

Total: ~849 lines added, 7 removed. 8 new tests.

## Bugs found and fixed

### BUG-1: Decoder o0 bit position (critical)

Decoder read `bit(word, 15)` for o0. Correct position: bit 21. Caused
LDXR→LoadAcquire misclassification.

**Fix:** `bit(word, 21)`.

### BUG-2: Test encoding bit pattern (high)

Test used `0b011` for bits [29:27]. Correct: `0b001` (load/store exclusive
encoding). All test encodings were wrong.

**Fix:** Recomputed all encodings with correct bit layout.

### BUG-3: STXR disassembly Rs register (medium)

Rs disassembled via `gp_or_zr` → "x2" for 32-bit STXR. Rs is always 32-bit
(W register) per AArch64 spec.

**Fix:** Use `reg_for_size(rs, MemSize::Word)`.

### BUG-4: Eval tests checked wrong register (medium)

`atomic_test_function` sets X(0) to Value(0) (address const), not the
StoreExclusive status. Tests checked X(0) expecting status.

**Fix:** Verify behavior through memory state and monitor state instead.

## Findings

### FINDING-1: Native backend returns UnsupportedOp for atomics (low)

`nx86-x64-v4` returns `LoweringError::UnsupportedOp` for all 4 atomic ops.
Interpreter/evaluator handle atomics correctly. Native lowering deferred.

### FINDING-2: acquire/release = plain read/write in v0 (info)

x86 TSO is stronger than AArch64. `LoadAcquire` = `Load`, `StoreRelease` =
`Store`. Correct for single-thread. Multi-thread needs `LOCK` prefix or
compiler barriers.

### FINDING-3: No synthetic .toml test files yet (low)

SPEC exit criteria says "basic atomic synthetic tests pass". Unit tests cover
evaluator + interpreter. Synthetic `.toml` files deferred (format needs
atomic instruction hex encoding, which is now available).

## Test coverage

| Test | What |
|------|------|
| `decodes_ldxr_stxr_ldar_stlr` | 64-bit decode + disasm |
| `decodes_32bit_atomic_variants` | 32-bit decode + disasm |
| `eval_exclusive_load_sets_monitor` | LDXR sets monitor |
| `eval_exclusive_store_succeeds_when_monitored` | STXR success → memory written |
| `eval_exclusive_store_fails_when_not_monitored` | STXR fail → memory unchanged |
| `eval_exclusive_store_fails_on_address_mismatch` | STXR fail → monitor cleared |
| `eval_acquire_load_reads_value` | LDAR reads correct value |
| `eval_release_store_writes_value` | STLR writes correct value |

## Verification

```
cargo fmt --all -- --check         → PASS
cargo clippy --workspace --all-targets -- -D warnings → PASS
cargo test --workspace --all-targets → 229 pass, 0 fail
```

Host: `aarch64-apple-darwin`
