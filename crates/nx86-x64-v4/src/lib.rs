use nx86_core::guest::{CpuState, Nzcv};
use nx86_ir::verify::{self, VerifyError};
use nx86_ir::{BinaryOp, Function, Inst, Op, Reg, Terminator, Type, Value};
use nx86_x64_asm::{AsmError, Assembler, CodeBuffer, Mem64, Reg64};
use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-x64-v4";

const X0_OFFSET: i32 = 0;
const SP_OFFSET: i32 = X0_OFFSET + 31 * 8;
const PC_OFFSET: i32 = SP_OFFSET + 8;
const NZCV_BITS_OFFSET: i32 = PC_OFFSET + 8;
const HALTED_OFFSET: i32 = NZCV_BITS_OFFSET + 8;

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredBlock {
    code: CodeBuffer,
    stack_size: i32,
}

impl LoweredBlock {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.code.bytes()
    }

    #[must_use]
    pub fn dump(&self) -> &str {
        self.code.dump()
    }

    #[must_use]
    pub const fn stack_size(&self) -> i32 {
        self.stack_size
    }

    #[must_use]
    pub const fn code(&self) -> &CodeBuffer {
        &self.code
    }
}

#[repr(C)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeBlockState {
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub nzcv_bits: u64,
    pub halted: u64,
}

impl NativeBlockState {
    #[must_use]
    pub fn from_cpu_state(cpu: &CpuState) -> Self {
        Self {
            x: *cpu.general_registers(),
            sp: cpu.sp(),
            pc: cpu.pc(),
            nzcv_bits: u64::from(cpu.nzcv().bits()),
            halted: u64::from(cpu.halted()),
        }
    }

    #[must_use]
    pub fn to_cpu_state(&self, halt_reason: Option<&str>) -> CpuState {
        self.apply_to_cpu_state(CpuState::new(), halt_reason)
    }

    #[must_use]
    pub fn apply_to_cpu_state(&self, mut cpu: CpuState, halt_reason: Option<&str>) -> CpuState {
        for (index, value) in self.x.iter().copied().enumerate() {
            cpu.set_x(index as u8, value);
        }
        cpu.set_sp(self.sp);
        cpu.set_pc(self.pc);
        cpu.set_nzcv(Nzcv::from_bits(self.nzcv_bits as u32));
        if self.halted != 0 {
            cpu.halt(halt_reason.unwrap_or("native block halted"));
        } else {
            cpu.clear_halt();
        }
        cpu
    }
}

#[derive(Debug, Error)]
pub enum LoweringError {
    #[error("input function failed NxIR verification: {0}")]
    InvalidIr(#[from] VerifyError),
    #[error("tiny native lowering supports exactly one block, got {count}")]
    UnsupportedBlockCount { count: usize },
    #[error("tiny native lowering does not support {op}")]
    UnsupportedOp { op: &'static str },
    #[error("tiny native lowering does not support {terminator}")]
    UnsupportedTerminator { terminator: &'static str },
    #[error("instruction at {guest_address:#x} is missing result value for {op}")]
    MissingResult {
        guest_address: u64,
        op: &'static str,
    },
    #[error("value {value:?} is outside the function value table")]
    ValueOutOfRange { value: Value },
    #[error("guest register x{register} is outside the native state")]
    RegisterOutOfRange { register: u8 },
    #[error("stack frame for {value_count} SSA values is too large")]
    StackTooLarge { value_count: u32 },
    #[error("terminator address {address:#x} overflows when advancing PC")]
    AddressOverflow { address: u64 },
    #[error("assembler failed: {0}")]
    Assembler(#[from] AsmError),
}

pub fn lower_tiny_block(function: &Function) -> Result<LoweredBlock, LoweringError> {
    verify::verify(function)?;
    if function.blocks.len() != 1 {
        return Err(LoweringError::UnsupportedBlockCount {
            count: function.blocks.len(),
        });
    }

    let stack_size = stack_size(function.value_count)?;
    let block = &function.blocks[0];
    let mut asm = Assembler::new();
    asm.prologue();
    if stack_size > 0 {
        asm.sub_reg_imm32(Reg64::Rsp, stack_size);
    }
    asm.mov_reg_imm64(Reg64::Rax, 0);
    asm.mov_mem_reg(state_mem(HALTED_OFFSET), Reg64::Rax);

    for inst in &block.instructions {
        lower_inst(&mut asm, inst, function.value_count)?;
    }

    match &block.terminator {
        Terminator::Halt { .. } => {
            let next_pc =
                block
                    .terminator_address
                    .checked_add(4)
                    .ok_or(LoweringError::AddressOverflow {
                        address: block.terminator_address,
                    })?;
            asm.mov_reg_imm64(Reg64::Rax, next_pc);
            asm.mov_mem_reg(state_mem(PC_OFFSET), Reg64::Rax);
            asm.mov_reg_imm64(Reg64::Rax, 1);
            asm.mov_mem_reg(state_mem(HALTED_OFFSET), Reg64::Rax);
        }
        Terminator::Return => {}
        Terminator::Branch { .. } => {
            return Err(LoweringError::UnsupportedTerminator {
                terminator: "branch",
            });
        }
        Terminator::CondBranch { .. } => {
            return Err(LoweringError::UnsupportedTerminator {
                terminator: "conditional branch",
            });
        }
    }

    if stack_size > 0 {
        asm.add_reg_imm32(Reg64::Rsp, stack_size);
    }
    asm.epilogue();

    let code = asm.finish()?;
    Ok(LoweredBlock { code, stack_size })
}

fn lower_inst(asm: &mut Assembler, inst: &Inst, value_count: u32) -> Result<(), LoweringError> {
    match &inst.op {
        Op::Const {
            ty: Type::I64,
            value,
        } => {
            let result = result_value(inst, "const")?;
            asm.mov_reg_imm64(Reg64::Rax, *value);
            store_value(asm, result, value_count)?;
        }
        Op::GetReg { reg } => {
            let result = result_value(inst, "getreg")?;
            load_reg(asm, *reg)?;
            store_value(asm, result, value_count)?;
        }
        Op::SetReg { reg, value } => {
            if matches!(reg, Reg::X(31)) {
                return Ok(());
            }
            load_value(asm, *value, value_count, Reg64::Rax)?;
            store_reg(asm, *reg)?;
        }
        Op::Binary {
            op,
            ty: Type::I64,
            lhs,
            rhs,
        } => {
            let result = result_value(inst, "binary")?;
            load_value(asm, *lhs, value_count, Reg64::Rax)?;
            load_value(asm, *rhs, value_count, Reg64::Rcx)?;
            match op {
                BinaryOp::Add => asm.add_reg_reg(Reg64::Rax, Reg64::Rcx),
                BinaryOp::Sub => asm.sub_reg_reg(Reg64::Rax, Reg64::Rcx),
                BinaryOp::And | BinaryOp::Or | BinaryOp::Xor => {
                    return Err(LoweringError::UnsupportedOp {
                        op: "logical binary operation",
                    });
                }
            }
            store_value(asm, result, value_count)?;
        }
        Op::Const { .. } => {
            return Err(LoweringError::UnsupportedOp {
                op: "non-i64 const",
            });
        }
        Op::Binary { .. } => {
            return Err(LoweringError::UnsupportedOp {
                op: "non-i64 binary operation",
            });
        }
        Op::Trunc { .. } | Op::ZeroExtend { .. } => {
            return Err(LoweringError::UnsupportedOp {
                op: "integer width conversion",
            });
        }
        Op::Load { .. } | Op::Store { .. } => {
            return Err(LoweringError::UnsupportedOp {
                op: "guest memory operation",
            });
        }
        Op::SetFlags { .. } => {
            return Err(LoweringError::UnsupportedOp { op: "lazy flags" });
        }
    }

    Ok(())
}

fn result_value(inst: &Inst, op: &'static str) -> Result<Value, LoweringError> {
    inst.result.ok_or(LoweringError::MissingResult {
        guest_address: inst.guest_address,
        op,
    })
}

fn stack_size(value_count: u32) -> Result<i32, LoweringError> {
    let bytes = u64::from(value_count)
        .checked_mul(8)
        .ok_or(LoweringError::StackTooLarge { value_count })?;
    let aligned = bytes
        .checked_add(15)
        .map(|value| value & !15)
        .ok_or(LoweringError::StackTooLarge { value_count })?;
    i32::try_from(aligned).map_err(|_| LoweringError::StackTooLarge { value_count })
}

fn value_slot(value: Value, value_count: u32) -> Result<Mem64, LoweringError> {
    if value.0 >= value_count {
        return Err(LoweringError::ValueOutOfRange { value });
    }
    let offset = i32::try_from((u64::from(value.0) + 1) * 8)
        .map_err(|_| LoweringError::StackTooLarge { value_count })?;
    Ok(Mem64::new(Reg64::Rbp, -offset))
}

fn load_value(
    asm: &mut Assembler,
    value: Value,
    value_count: u32,
    target: Reg64,
) -> Result<(), LoweringError> {
    asm.mov_reg_mem(target, value_slot(value, value_count)?);
    Ok(())
}

fn store_value(asm: &mut Assembler, value: Value, value_count: u32) -> Result<(), LoweringError> {
    asm.mov_mem_reg(value_slot(value, value_count)?, Reg64::Rax);
    Ok(())
}

fn load_reg(asm: &mut Assembler, reg: Reg) -> Result<(), LoweringError> {
    match reg {
        Reg::X(31) => asm.mov_reg_imm64(Reg64::Rax, 0),
        Reg::X(index) if index < 31 => asm.mov_reg_mem(Reg64::Rax, state_mem(x_offset(index))),
        Reg::X(register) => return Err(LoweringError::RegisterOutOfRange { register }),
        Reg::Sp => asm.mov_reg_mem(Reg64::Rax, state_mem(SP_OFFSET)),
    }
    Ok(())
}

fn store_reg(asm: &mut Assembler, reg: Reg) -> Result<(), LoweringError> {
    match reg {
        Reg::X(31) => {}
        Reg::X(index) if index < 31 => asm.mov_mem_reg(state_mem(x_offset(index)), Reg64::Rax),
        Reg::X(register) => return Err(LoweringError::RegisterOutOfRange { register }),
        Reg::Sp => asm.mov_mem_reg(state_mem(SP_OFFSET), Reg64::Rax),
    }
    Ok(())
}

const fn x_offset(index: u8) -> i32 {
    X0_OFFSET + (index as i32 * 8)
}

const fn state_mem(offset: i32) -> Mem64 {
    Mem64::new(Reg64::Rdi, offset)
}

#[cfg(test)]
mod tests {
    use nx86_ir::{Block, Function, Inst, Op, Reg, Terminator, Type, Value};

    use super::{LoweringError, NativeBlockState, lower_tiny_block};

    #[test]
    fn lowers_tiny_add_block() {
        let function = tiny_add_function();

        let lowered = lower_tiny_block(&function).expect("tiny add should lower");

        assert!(!lowered.bytes().is_empty());
        assert_eq!(lowered.stack_size(), 32);
        assert!(lowered.dump().contains("mov [rdi+0x100], rax"));
        assert!(lowered.dump().contains("ret"));
    }

    #[test]
    fn rejects_logical_binary_ops() {
        let mut function = tiny_add_function();
        function.blocks[0].instructions[4].op = Op::Binary {
            op: nx86_ir::BinaryOp::And,
            ty: Type::I64,
            lhs: Value(1),
            rhs: Value(2),
        };

        let error = lower_tiny_block(&function).expect_err("logical op should be rejected");

        assert!(matches!(error, LoweringError::UnsupportedOp { .. }));
    }

    #[test]
    fn native_state_round_trips_cpu_state() {
        let mut cpu = nx86_core::guest::CpuState::new();
        cpu.set_x(0, 1);
        cpu.set_x(30, 30);
        cpu.set_sp(0x1000);
        cpu.set_pc(0x2000);
        cpu.halt("svc #0x0");

        let state = NativeBlockState::from_cpu_state(&cpu);
        let round_trip = state.to_cpu_state(Some("svc #0x0"));

        assert_eq!(round_trip, cpu);
    }

    #[test]
    fn native_state_apply_preserves_unmodelled_cpu_fields() {
        let mut cpu = nx86_core::guest::CpuState::new();
        cpu.set_vector(0, 0xfeed_beef);
        cpu.set_fpcr(0x1234);
        cpu.set_fpsr(0x5678);
        cpu.set_thread(nx86_core::guest::ThreadState {
            thread_id: 42,
            name: Some("worker".to_owned()),
            deterministic_index: 7,
        });
        cpu.halt("old halt");

        let mut state = NativeBlockState::from_cpu_state(&cpu);
        state.x[1] = 99;
        state.pc = 0x40;
        state.halted = 0;
        let applied = state.apply_to_cpu_state(cpu.clone(), None);

        assert_eq!(applied.x(1), 99);
        assert_eq!(applied.pc(), 0x40);
        assert!(!applied.halted());
        assert_eq!(applied.vector(0), cpu.vector(0));
        assert_eq!(applied.fpcr(), cpu.fpcr());
        assert_eq!(applied.fpsr(), cpu.fpsr());
        assert_eq!(applied.thread(), cpu.thread());
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    #[allow(unsafe_code)]
    fn calls_lowered_tiny_add_block() {
        let function = tiny_add_function();
        let lowered = lower_tiny_block(&function).expect("tiny add should lower");
        let executable =
            nx86_jit::ExecutableMemory::new(lowered.bytes()).expect("code should allocate");
        let cpu = nx86_core::guest::CpuState::new();
        let mut state = NativeBlockState::from_cpu_state(&cpu);

        // SAFETY: `lower_tiny_block` produced an `extern "C"
        // fn(*mut NativeBlockState)` body for this exact state layout.
        unsafe { executable.call_with_state(&mut state) }.expect("native block should run");
        let final_state = state.to_cpu_state(Some("svc #0x0"));

        assert_eq!(final_state.x(1), 3);
        assert_eq!(final_state.pc(), 12);
        assert!(final_state.halted());
        assert_eq!(final_state.halt_reason(), Some("svc #0x0"));
    }

    fn tiny_add_function() -> Function {
        Function {
            name: "tiny_add".to_owned(),
            entry_address: 0,
            value_count: 4,
            blocks: vec![Block {
                instructions: vec![
                    Inst {
                        result: Some(Value(0)),
                        op: Op::Const {
                            ty: Type::I64,
                            value: 1,
                        },
                        guest_address: 0,
                    },
                    Inst {
                        result: None,
                        op: Op::SetReg {
                            reg: Reg::X(0),
                            value: Value(0),
                        },
                        guest_address: 0,
                    },
                    Inst {
                        result: Some(Value(1)),
                        op: Op::GetReg { reg: Reg::X(0) },
                        guest_address: 4,
                    },
                    Inst {
                        result: Some(Value(2)),
                        op: Op::Const {
                            ty: Type::I64,
                            value: 2,
                        },
                        guest_address: 4,
                    },
                    Inst {
                        result: Some(Value(3)),
                        op: Op::Binary {
                            op: nx86_ir::BinaryOp::Add,
                            ty: Type::I64,
                            lhs: Value(1),
                            rhs: Value(2),
                        },
                        guest_address: 4,
                    },
                    Inst {
                        result: None,
                        op: Op::SetReg {
                            reg: Reg::X(1),
                            value: Value(3),
                        },
                        guest_address: 4,
                    },
                ],
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 8,
            }],
        }
    }
}
