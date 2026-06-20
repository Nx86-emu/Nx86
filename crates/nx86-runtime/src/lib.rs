use nx86_arm64_decode::{
    DecodeError, DecodedInstruction, InstructionKind, LogicalOp, MemSize, decode_program,
};
use nx86_arm64_lift::lift_program;
use nx86_core::guest::{CpuState, CpuStateDiff, Nzcv};
use nx86_testsuite::{Framebuffer, MemoryDiff, SyntheticArm64Test, SyntheticTestError};
use nx86_vmm::{GuestAddress, GuestMemory, PAGE_SIZE, PagePermissions, VmmFault};
use thiserror::Error;

mod eval;

pub use eval::{EvalError, evaluate};

#[derive(Clone, Debug)]
pub struct TinyInterpreter {
    instructions: Vec<DecodedInstruction>,
    base_address: u64,
    max_steps: usize,
}

impl TinyInterpreter {
    #[must_use]
    pub fn new(instructions: Vec<DecodedInstruction>) -> Self {
        let base_address = instructions
            .first()
            .map_or(0, |instruction| instruction.address);
        Self {
            instructions,
            base_address,
            max_steps: 1_000,
        }
    }

    #[must_use]
    pub const fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// Run the program against a fresh, empty guest memory.
    ///
    /// Programs that perform memory operations should use [`Self::run_in`] with
    /// a memory that has the relevant pages mapped.
    pub fn run(&self, state: CpuState) -> Result<InterpreterResult, InterpreterError> {
        let mut memory = GuestMemory::new_logical();
        self.run_in(state, &mut memory)
    }

    /// Run the program against a caller-provided guest memory so stores and
    /// loads are observable after execution.
    pub fn run_in(
        &self,
        mut state: CpuState,
        memory: &mut GuestMemory,
    ) -> Result<InterpreterResult, InterpreterError> {
        let mut trace = Vec::new();

        for _ in 0..self.max_steps {
            if state.halted() {
                return Ok(InterpreterResult {
                    final_state: state,
                    trace,
                });
            }

            let pc = state.pc();
            let instruction = self.instruction_at(pc)?;
            trace.push(TraceStep {
                pc,
                disassembly: instruction.disassembly.clone(),
            });
            self.execute(instruction, &mut state, memory)?;
        }

        Err(InterpreterError::StepLimit {
            max_steps: self.max_steps,
        })
    }

    fn instruction_at(&self, pc: u64) -> Result<&DecodedInstruction, InterpreterError> {
        let offset = pc
            .checked_sub(self.base_address)
            .ok_or(InterpreterError::PcOutOfProgram { pc })?;
        if offset % 4 != 0 {
            return Err(InterpreterError::PcOutOfProgram { pc });
        }
        let index =
            usize::try_from(offset / 4).map_err(|_| InterpreterError::PcOutOfProgram { pc })?;
        self.instructions
            .get(index)
            .ok_or(InterpreterError::PcOutOfProgram { pc })
    }

    fn execute(
        &self,
        instruction: &DecodedInstruction,
        state: &mut CpuState,
        memory: &mut GuestMemory,
    ) -> Result<(), InterpreterError> {
        let next_pc = instruction.address + 4;
        match instruction.kind {
            InstructionKind::MovZ { rd, imm, .. } => {
                state.set_x(rd, imm);
                state.set_pc(next_pc);
            }
            InstructionKind::AddImmediate { rd, rn, imm } => {
                let value = state.read_gp_or_sp(rn).wrapping_add(imm);
                state.write_gp_or_sp(rd, value);
                state.set_pc(next_pc);
            }
            InstructionKind::SubImmediate { rd, rn, imm } => {
                let value = state.read_gp_or_sp(rn).wrapping_sub(imm);
                state.write_gp_or_sp(rd, value);
                state.set_pc(next_pc);
            }
            InstructionKind::AddsImmediate { rd, rn, imm } => {
                let lhs = state.read_gp_or_sp(rn);
                state.set_nzcv(Nzcv::from_add(lhs, imm));
                state.set_x(rd, lhs.wrapping_add(imm));
                state.set_pc(next_pc);
            }
            InstructionKind::SubsImmediate { rd, rn, imm } => {
                let lhs = state.read_gp_or_sp(rn);
                state.set_nzcv(Nzcv::from_sub(lhs, imm));
                state.set_x(rd, lhs.wrapping_sub(imm));
                state.set_pc(next_pc);
            }
            InstructionKind::Branch { target, .. } => {
                if self.instruction_at(target).is_err() {
                    return Err(InterpreterError::BranchOutOfProgram {
                        pc: instruction.address,
                        target,
                    });
                }
                state.set_pc(target);
            }
            InstructionKind::CondBranch { cond, target, .. } => {
                if state.nzcv().satisfies(cond) {
                    if self.instruction_at(target).is_err() {
                        return Err(InterpreterError::BranchOutOfProgram {
                            pc: instruction.address,
                            target,
                        });
                    }
                    state.set_pc(target);
                } else {
                    state.set_pc(next_pc);
                }
            }
            InstructionKind::LogicalReg { op, rd, rn, rm } => {
                let lhs = state.x(rn);
                let rhs = state.x(rm);
                let value = match op {
                    LogicalOp::And => lhs & rhs,
                    LogicalOp::Or => lhs | rhs,
                    LogicalOp::Xor => lhs ^ rhs,
                };
                state.set_x(rd, value);
                state.set_pc(next_pc);
            }
            InstructionKind::Svc { imm } => {
                state.set_pc(next_pc);
                state.halt(format!("svc #{imm:#x}"));
            }
            InstructionKind::Store {
                rt,
                rn,
                offset,
                size,
            } => {
                let address = state.read_gp_or_sp(rn).wrapping_add(offset);
                let value = state.x(rt);
                match size {
                    MemSize::Word => {
                        memory.write(GuestAddress(address), &(value as u32).to_le_bytes())?;
                    }
                    MemSize::Double => {
                        memory.write(GuestAddress(address), &value.to_le_bytes())?;
                    }
                }
                state.set_pc(next_pc);
            }
            InstructionKind::Load {
                rt,
                rn,
                offset,
                size,
            } => {
                let address = state.read_gp_or_sp(rn).wrapping_add(offset);
                let value = match size {
                    MemSize::Word => {
                        let bytes = memory.read(GuestAddress(address), 4)?;
                        let mut word = [0u8; 4];
                        word.copy_from_slice(&bytes);
                        u64::from(u32::from_le_bytes(word))
                    }
                    MemSize::Double => {
                        let bytes = memory.read(GuestAddress(address), 8)?;
                        let mut word = [0u8; 8];
                        word.copy_from_slice(&bytes);
                        u64::from_le_bytes(word)
                    }
                };
                state.set_x(rt, value);
                state.set_pc(next_pc);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterpreterResult {
    pub final_state: CpuState,
    pub trace: Vec<TraceStep>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceStep {
    pub pc: u64,
    pub disassembly: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticRunResult {
    pub interpreter: InterpreterResult,
    pub register_diffs: Vec<CpuStateDiff>,
    pub memory_diffs: Vec<MemoryDiff>,
    pub framebuffer: Option<Framebuffer>,
    /// Result of lifting the program to NxIR and evaluating it as a differential
    /// cross-check against the AArch64 interpreter.
    pub nxir: NxirOutcome,
}

/// Outcome of the NxIR differential cross-check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NxirOutcome {
    /// Human-readable NxIR text dump (empty if lifting failed).
    pub dump: String,
    /// Whether the NxIR evaluation agrees with the interpreter on final state
    /// and observable memory.
    pub agrees: bool,
    /// Lift/verify/evaluation error, if any.
    pub error: Option<String>,
    /// Final guest state from NxIR evaluation, if it ran.
    pub final_state: Option<CpuState>,
}

impl NxirOutcome {
    fn failed(error: impl ToString) -> Self {
        Self {
            dump: String::new(),
            agrees: false,
            error: Some(error.to_string()),
            final_state: None,
        }
    }
}

pub fn run_synthetic_test(
    test: &SyntheticArm64Test,
) -> Result<SyntheticRunResult, InterpreterError> {
    let entry_point = test.entry_point()?;
    let instructions = decode_program(&test.program.bytes, entry_point)?;
    let mut state = CpuState::new();
    state.set_pc(entry_point);

    let mut memory = GuestMemory::new_logical();
    map_regions(&mut memory, test)?;

    let interpreter = TinyInterpreter::new(instructions.clone()).run_in(state, &mut memory)?;
    let register_diffs = test.expected.compare_cpu_state(&interpreter.final_state)?;
    let memory_diffs = compare_expected_memory(&memory, test)?;
    let framebuffer = read_framebuffer(&memory, test)?;

    let nxir = run_nxir_differential(
        test,
        &instructions,
        entry_point,
        &interpreter.final_state,
        &memory,
    );

    Ok(SyntheticRunResult {
        interpreter,
        register_diffs,
        memory_diffs,
        framebuffer,
        nxir,
    })
}

/// Lift the program to NxIR, verify it, evaluate it on a fresh memory, and
/// compare the result with the interpreter. This is best-effort: any failure is
/// captured in [`NxirOutcome::error`] rather than propagated, so the GUI still
/// shows interpreter results for programs the lifter cannot yet handle.
fn run_nxir_differential(
    test: &SyntheticArm64Test,
    instructions: &[DecodedInstruction],
    entry_point: u64,
    interpreter_state: &CpuState,
    interpreter_memory: &GuestMemory,
) -> NxirOutcome {
    let mut function = match lift_program("synthetic", instructions, entry_point) {
        Ok(function) => function,
        Err(error) => return NxirOutcome::failed(error),
    };

    // Run the dead-flag pass and re-verify (SPEC §21.5: the verifier runs after
    // every optimization pass).
    nx86_ir_opt::eliminate_dead_flags(&mut function);
    if let Err(error) = nx86_ir::verify::verify(&function) {
        return NxirOutcome::failed(error);
    }
    let dump = function.dump();

    let mut memory = GuestMemory::new_logical();
    if let Err(error) = map_regions(&mut memory, test) {
        return NxirOutcome {
            dump,
            agrees: false,
            error: Some(error.to_string()),
            final_state: None,
        };
    }

    let nxir_state = match evaluate(&function, &mut memory) {
        Ok(state) => state,
        Err(error) => {
            return NxirOutcome {
                dump,
                agrees: false,
                error: Some(error.to_string()),
                final_state: None,
            };
        }
    };

    let memory_agrees = match (
        read_observable_regions(interpreter_memory, test),
        read_observable_regions(&memory, test),
    ) {
        (Ok(interpreter_regions), Ok(nxir_regions)) => interpreter_regions == nxir_regions,
        _ => false,
    };
    let agrees = &nxir_state == interpreter_state && memory_agrees;

    NxirOutcome {
        dump,
        agrees,
        error: None,
        final_state: Some(nxir_state),
    }
}

/// Map any framebuffer and expected-memory regions read-write so a program can
/// store into them and the results can be read back afterward.
fn map_regions(
    memory: &mut GuestMemory,
    test: &SyntheticArm64Test,
) -> Result<(), InterpreterError> {
    if let Some(spec) = &test.framebuffer {
        map_region(memory, spec.base_u64()?, spec.byte_len())?;
    }
    for range in &test.expected.memory {
        map_region(memory, range.address_u64()?, range.bytes.len())?;
    }
    Ok(())
}

/// Read the framebuffer and expected-memory regions, used to compare what each
/// execution engine wrote to memory.
fn read_observable_regions(
    memory: &GuestMemory,
    test: &SyntheticArm64Test,
) -> Result<Vec<Vec<u8>>, InterpreterError> {
    let mut regions = Vec::new();
    if let Some(spec) = &test.framebuffer {
        regions.push(memory.read(GuestAddress(spec.base_u64()?), spec.byte_len())?);
    }
    for range in &test.expected.memory {
        regions.push(memory.read(GuestAddress(range.address_u64()?), range.bytes.len())?);
    }
    Ok(regions)
}

/// Map every 4 KiB page that overlaps `[base, base + len)` read-write.
fn map_region(memory: &mut GuestMemory, base: u64, len: usize) -> Result<(), InterpreterError> {
    if len == 0 {
        return Ok(());
    }
    let end = base.saturating_add(len as u64);
    let mut page = GuestAddress(base).page_base();
    while page < end {
        memory.map_page(GuestAddress(page), PagePermissions::READ_WRITE)?;
        page += PAGE_SIZE;
    }
    Ok(())
}

fn compare_expected_memory(
    memory: &GuestMemory,
    test: &SyntheticArm64Test,
) -> Result<Vec<MemoryDiff>, InterpreterError> {
    let mut diffs = Vec::new();
    for range in &test.expected.memory {
        let address = range.address_u64()?;
        let actual = memory.read(GuestAddress(address), range.bytes.len())?;
        if actual != range.bytes {
            diffs.push(MemoryDiff {
                address,
                expected: range.bytes.clone(),
                actual,
            });
        }
    }
    Ok(diffs)
}

fn read_framebuffer(
    memory: &GuestMemory,
    test: &SyntheticArm64Test,
) -> Result<Option<Framebuffer>, InterpreterError> {
    let Some(spec) = &test.framebuffer else {
        return Ok(None);
    };
    let bytes = memory.read(GuestAddress(spec.base_u64()?), spec.byte_len())?;
    Ok(Some(Framebuffer {
        width: spec.width,
        height: spec.height,
        bytes,
    }))
}

#[derive(Debug, Error)]
pub enum InterpreterError {
    #[error("decode error: {0}")]
    Decode(#[from] DecodeError),
    #[error("synthetic test error: {0}")]
    Synthetic(#[from] SyntheticTestError),
    #[error("memory fault: {0}")]
    Memory(#[from] VmmFault),
    #[error("pc {pc:#x} is outside the decoded program")]
    PcOutOfProgram { pc: u64 },
    #[error("branch at {pc:#x} targets {target:#x}, outside decoded program")]
    BranchOutOfProgram { pc: u64, target: u64 },
    #[error("interpreter exceeded step limit {max_steps}")]
    StepLimit { max_steps: usize },
}

#[cfg(test)]
mod tests {
    use nx86_arm64_decode::decode_program;
    use nx86_core::guest::CpuState;
    use nx86_testsuite::SyntheticArm64Test;

    use super::{InterpreterError, TinyInterpreter, run_synthetic_test};

    #[test]
    fn synthetic_add_program_executes_and_matches_expected_registers() {
        let test = SyntheticArm64Test::parse(
            r#"
            [metadata]
            name = "add"
            entry-point = "0x0"

            [program]
            arm64-hex = "20 00 80 D2 01 08 00 91 01 00 00 D4"

            [expected.registers]
            x0 = "0x1"
            x1 = "0x3"
            pc = "0xc"
            halted = "true"
            "#,
        )
        .expect("test should parse");

        let result = run_synthetic_test(&test).expect("test should run");

        assert!(result.register_diffs.is_empty());
        assert_eq!(result.interpreter.final_state.x(1), 3);
        assert_eq!(result.interpreter.trace.len(), 3);
    }

    #[test]
    fn branch_updates_pc_and_skips_instruction() {
        let bytes = [
            0x20, 0x00, 0x80, 0xD2, // mov x0, #1
            0x02, 0x00, 0x00, 0x14, // b +8
            0x40, 0x00, 0x80, 0xD2, // mov x0, #2
            0x01, 0x00, 0x00, 0xD4, // svc #0
        ];
        let decoded = decode_program(&bytes, 0).expect("program should decode");
        let mut state = CpuState::new();
        state.set_pc(0);

        let result = TinyInterpreter::new(decoded)
            .run(state)
            .expect("program should run");

        assert_eq!(result.final_state.x(0), 1);
        assert_eq!(result.final_state.pc(), 16);
        assert!(result.final_state.halted());
    }

    #[test]
    fn svc_halts_and_advances_pc() {
        let decoded =
            decode_program(&[0x01, 0x00, 0x00, 0xD4], 0x1000).expect("program should decode");
        let mut state = CpuState::new();
        state.set_pc(0x1000);

        let result = TinyInterpreter::new(decoded)
            .run(state)
            .expect("program should run");

        assert!(result.final_state.halted());
        assert_eq!(result.final_state.pc(), 0x1004);
    }

    #[test]
    fn expected_register_mismatch_is_reported() {
        let test = SyntheticArm64Test::parse(
            r#"
            [metadata]
            name = "mismatch"

            [program]
            arm64-hex = "20 00 80 D2 01 00 00 D4"

            [expected.registers]
            x0 = "0x2"
            "#,
        )
        .expect("test should parse");

        let result = run_synthetic_test(&test).expect("test should run");

        assert_eq!(result.register_diffs.len(), 1);
    }

    #[test]
    fn synthetic_program_draws_framebuffer() {
        // movz x0, #0xffff, lsl #16 ; movz x1, #1, lsl #16 ; str w0 to four
        // 2x2 pixels ; svc #0. x0 = 0xffff0000 stores little-endian as the RGBA
        // bytes 00 00 ff ff (opaque blue).
        let test = SyntheticArm64Test::parse(
            r#"
            [metadata]
            name = "draw"
            entry-point = "0x0"

            [program]
            arm64-hex = "E0 FF BF D2 21 00 A0 D2 20 00 00 B9 20 04 00 B9 20 08 00 B9 20 0C 00 B9 01 00 00 D4"

            [framebuffer]
            base = "0x10000"
            width = 2
            height = 2
            "#,
        )
        .expect("test should parse");

        let result = run_synthetic_test(&test).expect("test should run");
        let framebuffer = result.framebuffer.expect("framebuffer should be present");

        assert_eq!(framebuffer.width, 2);
        assert_eq!(framebuffer.height, 2);
        assert_eq!(framebuffer.bytes, [0x00, 0x00, 0xFF, 0xFF].repeat(4));
        assert!(result.interpreter.final_state.halted());
        assert!(result.memory_diffs.is_empty());
        assert!(result.nxir.error.is_none(), "{:?}", result.nxir.error);
        assert!(result.nxir.agrees);
    }

    #[test]
    fn interpreter_and_nxir_agree_on_integer_program() {
        // mov x0, #5 ; add x1, x0, #3 ; sub x2, x1, #1 ; orr x3, x1, x0 ; svc #0
        let test = SyntheticArm64Test::parse(
            r#"
            [metadata]
            name = "integer"
            entry-point = "0x0"

            [program]
            arm64-hex = "A0 00 80 D2 01 0C 00 91 22 04 00 D1 23 00 00 AA 01 00 00 D4"

            [expected.registers]
            x0 = "0x5"
            x1 = "0x8"
            x2 = "0x7"
            x3 = "0xd"
            halted = "true"
            "#,
        )
        .expect("test should parse");

        let result = run_synthetic_test(&test).expect("test should run");

        assert!(result.register_diffs.is_empty());
        assert!(result.nxir.error.is_none(), "{:?}", result.nxir.error);
        assert!(result.nxir.agrees);
        assert_eq!(
            result.nxir.final_state.as_ref(),
            Some(&result.interpreter.final_state)
        );
    }

    #[test]
    fn interpreter_and_nxir_agree_on_memory_program() {
        // mov x0, #0xab ; mov x1, #1, lsl #16 ; str x0, [x1] ; ldr x2, [x1] ; svc
        let test = SyntheticArm64Test::parse(
            r#"
            [metadata]
            name = "memory"
            entry-point = "0x0"

            [program]
            arm64-hex = "60 15 80 D2 21 00 A0 D2 20 00 00 F9 22 00 40 F9 01 00 00 D4"

            [expected.registers]
            x2 = "0xab"
            halted = "true"

            [[expected.memory]]
            address = "0x10000"
            bytes-hex = "AB 00 00 00 00 00 00 00"
            "#,
        )
        .expect("test should parse");

        let result = run_synthetic_test(&test).expect("test should run");

        assert!(result.register_diffs.is_empty());
        assert!(result.memory_diffs.is_empty());
        assert!(result.nxir.error.is_none(), "{:?}", result.nxir.error);
        assert!(result.nxir.agrees);
    }

    #[test]
    fn conditional_branch_taken_agrees_through_lazy_flags() {
        // mov x0,#5 ; cmp x0,#5 ; b.eq +8 ; mov x1,#2 ; svc  (eq taken, x1 stays 0)
        let test = SyntheticArm64Test::parse(
            r#"
            [metadata]
            name = "beq-taken"
            entry-point = "0x0"

            [program]
            arm64-hex = "A0 00 80 D2 1F 14 00 F1 40 00 00 54 41 00 80 D2 01 00 00 D4"

            [expected.registers]
            x0 = "0x5"
            x1 = "0x0"
            halted = "true"
            "#,
        )
        .expect("test should parse");

        let result = run_synthetic_test(&test).expect("test should run");

        assert!(result.register_diffs.is_empty());
        assert!(result.nxir.error.is_none(), "{:?}", result.nxir.error);
        assert!(result.nxir.agrees);
    }

    #[test]
    fn conditional_branch_not_taken_agrees_through_lazy_flags() {
        // mov x0,#5 ; cmp x0,#6 ; b.eq +8 ; mov x1,#2 ; svc  (eq not taken, x1=2)
        let test = SyntheticArm64Test::parse(
            r#"
            [metadata]
            name = "beq-not-taken"
            entry-point = "0x0"

            [program]
            arm64-hex = "A0 00 80 D2 1F 18 00 F1 40 00 00 54 41 00 80 D2 01 00 00 D4"

            [expected.registers]
            x0 = "0x5"
            x1 = "0x2"
            halted = "true"
            "#,
        )
        .expect("test should parse");

        let result = run_synthetic_test(&test).expect("test should run");

        assert!(result.register_diffs.is_empty());
        assert!(result.nxir.error.is_none(), "{:?}", result.nxir.error);
        assert!(result.nxir.agrees);
    }

    #[test]
    fn overwritten_flags_are_eliminated_and_still_agree() {
        // mov x0,#5 ; cmp x0,#6 ; cmp x0,#5 ; b.eq +8 ; mov x1,#9 ; svc
        // The first cmp's flags are overwritten; the dead-flag pass drops them.
        let test = SyntheticArm64Test::parse(
            r#"
            [metadata]
            name = "overwritten-flags"
            entry-point = "0x0"

            [program]
            arm64-hex = "A0 00 80 D2 1F 18 00 F1 1F 14 00 F1 40 00 00 54 21 01 80 D2 01 00 00 D4"

            [expected.registers]
            x0 = "0x5"
            x1 = "0x0"
            halted = "true"
            "#,
        )
        .expect("test should parse");

        let result = run_synthetic_test(&test).expect("test should run");

        assert!(result.register_diffs.is_empty());
        assert!(result.nxir.error.is_none(), "{:?}", result.nxir.error);
        assert!(result.nxir.agrees);
        // Both CMPs lift to SetFlags, but the dead-flag pass leaves only one.
        assert_eq!(result.nxir.dump.matches("setflags").count(), 1);
    }

    #[test]
    fn interpreter_and_nxir_agree_across_branches() {
        // mov x0, #1 ; b +8 ; mov x0, #2 ; svc #0  (the second mov is skipped)
        let test = SyntheticArm64Test::parse(
            r#"
            [metadata]
            name = "branch"
            entry-point = "0x0"

            [program]
            arm64-hex = "20 00 80 D2 02 00 00 14 40 00 80 D2 01 00 00 D4"

            [expected.registers]
            x0 = "0x1"
            halted = "true"
            "#,
        )
        .expect("test should parse");

        let result = run_synthetic_test(&test).expect("test should run");

        assert!(result.register_diffs.is_empty());
        assert!(result.nxir.error.is_none(), "{:?}", result.nxir.error);
        assert!(result.nxir.agrees);
    }

    #[test]
    fn branch_out_of_program_errors() {
        let decoded = decode_program(&[0x04, 0x00, 0x00, 0x14], 0).expect("program should decode");
        let state = CpuState::new();

        let error = TinyInterpreter::new(decoded)
            .run(state)
            .expect_err("branch should fail");

        assert!(matches!(error, InterpreterError::BranchOutOfProgram { .. }));
    }
}
