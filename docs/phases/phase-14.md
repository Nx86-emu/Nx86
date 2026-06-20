# Phase 14: Core Integer Lifter

Phase 14 fills `nx86-arm64-lift`, which lifts decoded AArch64 into NxIR, and adds
an NxIR evaluator in `nx86-runtime` for differential testing.

The decoder gains the instructions the lifter needs: 32/64-bit `LDR`/`STR`
(unsigned offset) and logical-register `AND`/`ORR`/`EOR`. The lifter handles
MOV, ADD/SUB immediate, logical ops, unconditional branches, loads/stores, and
the synthetic `SVC` exit. It builds a basic-block CFG from branch targets, keeps
SSA values block-local (guest state crosses blocks via `GetReg`/`SetReg`), and
runs the Phase 13 verifier on its output.

`nx86-runtime::evaluate` is a reference interpreter over verified NxIR that
produces a `CpuState`. `run_synthetic_test` now runs both the AArch64
interpreter and the NxIR evaluator and reports whether they agree on the final
guest state and observable memory — the differential oracle from `SPEC.md`
§38.3. The GUI Tests screen shows the NxIR dump and the agreement status.

## Exit Criteria

- Synthetic integer programs (arithmetic, logical, loads/stores, branches) lift
  to NxIR and pass verification.
- The AArch64 interpreter and the NxIR evaluator agree on final registers,
  `pc`, halt state, and observable memory.
- The GUI displays the lifted NxIR and whether it matches the interpreter.
