use nx86_core::guest::{CpuState, Nzcv};
use nx86_ir::verify::{self, VerifyError};
use nx86_ir::{BinaryOp, Block, BlockId, Function, Inst, Op, Reg, Terminator, Type, Value};
use nx86_regalloc::{Allocation, Location, allocate};
use nx86_x64_asm::{AsmError, Assembler, CodeBuffer, Mem64, PatchKind, Reg64};
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
    chain_exits: Vec<ChainExit>,
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

    /// Patchable block-chain exits in this block (one per unconditional branch).
    #[must_use]
    pub fn chain_exits(&self) -> &[ChainExit] {
        &self.chain_exits
    }
}

/// The kind of block exit a [`ChainExit`] patches. v0 only chains unconditional
/// branches; conditional and guard exits are not yet patchable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainExitKind {
    UnconditionalBranch,
}

/// Metadata describing a patchable block-chain exit: enough to install a direct
/// `jmp` to the successor and to restore the original bytes on invalidation
/// (SPEC §23.3 / §26). The block is identified by its guest entry PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainExit {
    /// Guest entry PC of the block this exit belongs to.
    pub block_entry_pc: u64,
    pub exit_kind: ChainExitKind,
    /// Byte offset of the patch slot within the block's code.
    pub patch_offset: usize,
    /// Slot size (a `jmp rel32` is [`nx86_x64_asm::CHAIN_EXIT_SIZE`] bytes).
    pub patch_size: usize,
    /// The unpatched bytes (`ret` + padding), restored when the chain is broken.
    pub original_bytes: Vec<u8>,
    /// Guest entry PC of the branch's successor block.
    pub successor_pc: u64,
    /// Whether this exit is currently patched to a direct jump.
    pub patched: bool,
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
    #[error("branch target {target:?} is outside the function block table")]
    UnknownBranchTarget { target: BlockId },
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

/// A single block of a function lowered to native code, keyed by the guest PC
/// the dispatcher uses to reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredFunctionBlock {
    /// Guest entry PC of the block (its dispatcher key).
    pub entry_pc: u64,
    /// The block's lowered native code.
    pub lowered: LoweredBlock,
}

/// Lower a verified single-block function. Branches are rejected because a lone
/// block has no sibling to route to; use [`lower_function`] for multi-block
/// functions.
pub fn lower_tiny_block(function: &Function) -> Result<LoweredBlock, LoweringError> {
    verify::verify(function)?;
    if function.blocks.len() != 1 {
        return Err(LoweringError::UnsupportedBlockCount {
            count: function.blocks.len(),
        });
    }
    lower_block(
        &function.blocks[0],
        function.value_count,
        |_target: BlockId| {
            Err(LoweringError::UnsupportedTerminator {
                terminator: "branch",
            })
        },
    )
}

/// Lower every block of a verified function into a separately-callable native
/// block keyed by its guest entry PC. Unconditional branches set the next guest
/// PC and exit to the dispatcher; `Halt` additionally sets the halted flag.
pub fn lower_function(function: &Function) -> Result<Vec<LoweredFunctionBlock>, LoweringError> {
    verify::verify(function)?;
    if function.blocks.is_empty() {
        return Err(LoweringError::UnsupportedBlockCount { count: 0 });
    }

    let entry_pcs: Vec<u64> = function.blocks.iter().map(block_entry_pc).collect();
    let mut blocks = Vec::with_capacity(function.blocks.len());
    for (index, block) in function.blocks.iter().enumerate() {
        let lowered = lower_block_with_entries(block, function.value_count, &entry_pcs)?;
        blocks.push(LoweredFunctionBlock {
            entry_pc: entry_pcs[index],
            lowered,
        });
    }
    Ok(blocks)
}

/// Lower one block selected by guest entry PC from a verified function.
///
/// Returns `Ok(None)` when the function has no block at `entry_pc`. Branches
/// from the selected block still resolve against the complete function, so the
/// emitted block uses the same routing protocol as [`lower_function`].
pub fn lower_function_block(
    function: &Function,
    entry_pc: u64,
) -> Result<Option<LoweredFunctionBlock>, LoweringError> {
    verify::verify(function)?;
    let entry_pcs: Vec<u64> = function.blocks.iter().map(block_entry_pc).collect();
    let Some(index) = entry_pcs
        .iter()
        .position(|candidate| *candidate == entry_pc)
    else {
        return Ok(None);
    };
    let lowered =
        lower_block_with_entries(&function.blocks[index], function.value_count, &entry_pcs)?;
    Ok(Some(LoweredFunctionBlock { entry_pc, lowered }))
}

fn lower_block_with_entries(
    block: &Block,
    value_count: u32,
    entry_pcs: &[u64],
) -> Result<LoweredBlock, LoweringError> {
    lower_block(block, value_count, |target: BlockId| {
        entry_pcs
            .get(target.0 as usize)
            .copied()
            .ok_or(LoweringError::UnknownBranchTarget { target })
    })
}

/// Guest entry PC of a block: the address of its first instruction, or its
/// terminator address when the block has no instructions.
#[must_use]
pub fn block_entry_pc(block: &Block) -> u64 {
    block.entry_address()
}

/// Lower one block to a self-contained native block. `resolve_target` maps a
/// branch's `BlockId` to the guest PC the dispatcher should resume at.
fn lower_block<F>(
    block: &Block,
    value_count: u32,
    resolve_target: F,
) -> Result<LoweredBlock, LoweringError>
where
    F: Fn(BlockId) -> Result<u64, LoweringError>,
{
    let allocation = allocate(block, value_count);
    let stack_size = stack_size(allocation.spill_count())?;
    let mut asm = Assembler::new();
    asm.prologue();
    if stack_size > 0 {
        asm.sub_reg_imm32(Reg64::Rsp, stack_size);
    }
    // A block that exits via a branch leaves the halted flag clear so the
    // dispatcher routes to the next guest PC; `Halt` sets it in the terminator.
    asm.mov_reg_imm64(Reg64::Rax, 0);
    asm.mov_mem_reg(state_mem(HALTED_OFFSET), Reg64::Rax);

    for inst in &block.instructions {
        lower_inst(&mut asm, inst, &allocation)?;
    }

    let chain_successor = emit_terminator(&mut asm, block, resolve_target)?;

    if stack_size > 0 {
        asm.add_reg_imm32(Reg64::Rsp, stack_size);
    }
    // An unconditional branch tears down its frame and ends in a patchable
    // chain-exit slot (so a hot edge can later jump straight to its successor);
    // every other terminator uses the plain `ret` epilogue. The slot must come
    // after both `add rsp` and `pop rbp` so a chained jump stays stack-balanced.
    match chain_successor {
        Some(_) => asm.chain_epilogue(),
        None => asm.epilogue(),
    }

    let code = asm.finish()?;
    let chain_exits = build_chain_exits(&code, block.entry_address(), chain_successor);
    Ok(LoweredBlock {
        code,
        stack_size,
        chain_exits,
    })
}

/// Build the chain-exit metadata for a lowered block: a single entry for an
/// unconditional branch (the `chain_successor`), none otherwise.
fn build_chain_exits(
    code: &CodeBuffer,
    block_entry_pc: u64,
    chain_successor: Option<u64>,
) -> Vec<ChainExit> {
    let Some(successor_pc) = chain_successor else {
        return Vec::new();
    };
    code.patch_sites()
        .iter()
        .filter(|site| site.kind == PatchKind::ChainExit)
        .map(|site| {
            let original_bytes = code.bytes()[site.offset..site.offset + site.size].to_vec();
            ChainExit {
                block_entry_pc,
                exit_kind: ChainExitKind::UnconditionalBranch,
                patch_offset: site.offset,
                patch_size: site.size,
                original_bytes,
                successor_pc,
                patched: false,
            }
        })
        .collect()
}

/// Emit the terminator. Returns the successor guest PC when the block ends in an
/// unconditional branch (a chainable exit), `None` otherwise.
fn emit_terminator<F>(
    asm: &mut Assembler,
    block: &Block,
    resolve_target: F,
) -> Result<Option<u64>, LoweringError>
where
    F: Fn(BlockId) -> Result<u64, LoweringError>,
{
    match &block.terminator {
        Terminator::Halt { .. } => {
            let next_pc =
                block
                    .terminator_address
                    .checked_add(4)
                    .ok_or(LoweringError::AddressOverflow {
                        address: block.terminator_address,
                    })?;
            emit_set_pc(asm, next_pc);
            asm.mov_reg_imm64(Reg64::Rax, 1);
            asm.mov_mem_reg(state_mem(HALTED_OFFSET), Reg64::Rax);
            Ok(None)
        }
        Terminator::Branch { target } => {
            // Keep the PC store: unpatched, the dispatcher reads it; chained, it
            // leaves `native.pc == successor.entry_pc` on entry to the successor.
            let target_pc = resolve_target(*target)?;
            emit_set_pc(asm, target_pc);
            Ok(Some(target_pc))
        }
        Terminator::Return => Ok(None),
        Terminator::CondBranch { .. } => Err(LoweringError::UnsupportedTerminator {
            terminator: "conditional branch",
        }),
        Terminator::Guard { .. } => Err(LoweringError::UnsupportedTerminator {
            terminator: "guard",
        }),
    }
}

fn emit_set_pc(asm: &mut Assembler, pc: u64) {
    asm.mov_reg_imm64(Reg64::Rax, pc);
    asm.mov_mem_reg(state_mem(PC_OFFSET), Reg64::Rax);
}

fn lower_inst(asm: &mut Assembler, inst: &Inst, alloc: &Allocation) -> Result<(), LoweringError> {
    match &inst.op {
        Op::Const {
            ty: Type::I64,
            value,
        } => {
            let result = result_value(inst, "const")?;
            define_value(asm, result, alloc, |asm, dst| {
                asm.mov_reg_imm64(dst, *value);
                Ok(())
            })?;
        }
        Op::GetReg { reg } => {
            let result = result_value(inst, "getreg")?;
            define_value(asm, result, alloc, |asm, dst| {
                emit_load_guest(asm, dst, *reg)
            })?;
        }
        Op::SetReg { reg, value } => {
            if matches!(reg, Reg::X(31)) {
                return Ok(());
            }
            match location(alloc, *value)? {
                Location::Register(src) => emit_store_guest(asm, src, *reg)?,
                Location::Spill(slot) => {
                    asm.mov_reg_mem(Reg64::Rax, spill_slot(slot)?);
                    emit_store_guest(asm, Reg64::Rax, *reg)?;
                }
            }
        }
        Op::Binary {
            op,
            ty: Type::I64,
            lhs,
            rhs,
        } => {
            let result = result_value(inst, "binary")?;
            materialize_value(asm, *lhs, alloc, Reg64::Rax)?;
            materialize_value(asm, *rhs, alloc, Reg64::Rcx)?;
            match op {
                BinaryOp::Add => asm.add_reg_reg(Reg64::Rax, Reg64::Rcx),
                BinaryOp::Sub => asm.sub_reg_reg(Reg64::Rax, Reg64::Rcx),
                BinaryOp::And => asm.and_reg_reg(Reg64::Rax, Reg64::Rcx),
                BinaryOp::Or => asm.or_reg_reg(Reg64::Rax, Reg64::Rcx),
                BinaryOp::Xor => asm.xor_reg_reg(Reg64::Rax, Reg64::Rcx),
            }
            store_into_value(asm, result, alloc, Reg64::Rax)?;
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

fn stack_size(spill_count: u32) -> Result<i32, LoweringError> {
    let bytes = u64::from(spill_count)
        .checked_mul(8)
        .ok_or(LoweringError::StackTooLarge {
            value_count: spill_count,
        })?;
    let aligned =
        bytes
            .checked_add(15)
            .map(|value| value & !15)
            .ok_or(LoweringError::StackTooLarge {
                value_count: spill_count,
            })?;
    i32::try_from(aligned).map_err(|_| LoweringError::StackTooLarge {
        value_count: spill_count,
    })
}

fn location(alloc: &Allocation, value: Value) -> Result<Location, LoweringError> {
    alloc
        .location(value)
        .ok_or(LoweringError::ValueOutOfRange { value })
}

fn spill_slot(slot: u32) -> Result<Mem64, LoweringError> {
    let offset = i32::try_from((u64::from(slot) + 1) * 8)
        .map_err(|_| LoweringError::StackTooLarge { value_count: slot })?;
    Ok(Mem64::new(Reg64::Rbp, -offset))
}

/// Emit code that produces a result value into its assigned location. `emit`
/// writes the value into the register it is handed: for a register-allocated
/// result that register is its final home; for a spilled result it is the RAX
/// scratch, which is then stored to the value's stack slot.
fn define_value<F>(
    asm: &mut Assembler,
    value: Value,
    alloc: &Allocation,
    emit: F,
) -> Result<(), LoweringError>
where
    F: FnOnce(&mut Assembler, Reg64) -> Result<(), LoweringError>,
{
    match location(alloc, value)? {
        Location::Register(reg) => emit(asm, reg)?,
        Location::Spill(slot) => {
            emit(asm, Reg64::Rax)?;
            asm.mov_mem_reg(spill_slot(slot)?, Reg64::Rax);
        }
    }
    Ok(())
}

/// Load `value` into `dst`, whether it lives in a register or a spill slot.
fn materialize_value(
    asm: &mut Assembler,
    value: Value,
    alloc: &Allocation,
    dst: Reg64,
) -> Result<(), LoweringError> {
    match location(alloc, value)? {
        Location::Register(reg) => asm.mov_reg_reg(dst, reg),
        Location::Spill(slot) => asm.mov_reg_mem(dst, spill_slot(slot)?),
    }
    Ok(())
}

/// Store `src` into `value`'s assigned location.
fn store_into_value(
    asm: &mut Assembler,
    value: Value,
    alloc: &Allocation,
    src: Reg64,
) -> Result<(), LoweringError> {
    match location(alloc, value)? {
        Location::Register(reg) => asm.mov_reg_reg(reg, src),
        Location::Spill(slot) => asm.mov_mem_reg(spill_slot(slot)?, src),
    }
    Ok(())
}

fn emit_load_guest(asm: &mut Assembler, dst: Reg64, reg: Reg) -> Result<(), LoweringError> {
    match reg {
        Reg::X(31) => asm.mov_reg_imm64(dst, 0),
        Reg::X(index) if index < 31 => asm.mov_reg_mem(dst, state_mem(x_offset(index))),
        Reg::X(register) => return Err(LoweringError::RegisterOutOfRange { register }),
        Reg::Sp => asm.mov_reg_mem(dst, state_mem(SP_OFFSET)),
    }
    Ok(())
}

fn emit_store_guest(asm: &mut Assembler, src: Reg64, reg: Reg) -> Result<(), LoweringError> {
    match reg {
        Reg::X(31) => {}
        Reg::X(index) if index < 31 => asm.mov_mem_reg(state_mem(x_offset(index)), src),
        Reg::X(register) => return Err(LoweringError::RegisterOutOfRange { register }),
        Reg::Sp => asm.mov_mem_reg(state_mem(SP_OFFSET), src),
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
    use nx86_ir::{Block, BlockId, Function, Inst, Op, Reg, Terminator, Type, Value};

    use super::{NativeBlockState, lower_function, lower_function_block, lower_tiny_block};

    #[test]
    fn lowers_tiny_add_block() {
        let function = tiny_add_function();

        let lowered = lower_tiny_block(&function).expect("tiny add should lower");

        assert!(!lowered.bytes().is_empty());
        // The four SSA values all fit in pool registers, so nothing spills.
        assert_eq!(lowered.stack_size(), 0);
        assert!(lowered.dump().contains("mov rdx, 0x1"));
        assert!(lowered.dump().contains("mov [rdi+0x100], rax"));
        assert!(lowered.dump().contains("ret"));
    }

    #[test]
    fn lowers_logical_binary_ops() {
        let mut function = tiny_add_function();
        function.blocks[0].instructions[4].op = Op::Binary {
            op: nx86_ir::BinaryOp::And,
            ty: Type::I64,
            lhs: Value(1),
            rhs: Value(2),
        };

        let lowered = lower_tiny_block(&function).expect("logical op should lower");

        assert!(lowered.dump().contains("and rax, rcx"));
    }

    #[test]
    fn lower_function_routes_branch_to_target_pc() {
        let function = two_block_branch_function();
        let blocks = lower_function(&function).expect("two-block function should lower");

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].entry_pc, 0x0);
        assert_eq!(blocks[1].entry_pc, 0x8);

        // Block 0 branches: it sets PC to block 1's entry (0x8) and does not halt.
        let branch_dump = blocks[0].lowered.dump();
        assert!(branch_dump.contains("mov rax, 0x8"));
        assert!(branch_dump.contains("mov [rdi+0x100], rax"));
        assert!(!branch_dump.contains("mov rax, 0x1"));

        // Block 1 halts: it sets the halted flag (offset 0x110) to 1.
        let halt_dump = blocks[1].lowered.dump();
        assert!(halt_dump.contains("mov rax, 0x1"));
        assert!(halt_dump.contains("mov [rdi+0x110], rax"));
    }

    #[test]
    fn branch_block_exposes_a_chain_exit() {
        let function = two_block_branch_function();
        let blocks = lower_function(&function).expect("two-block function should lower");

        // The branch block has one chain exit to block 1's entry PC.
        let exits = blocks[0].lowered.chain_exits();
        assert_eq!(exits.len(), 1);
        let exit = &exits[0];
        assert_eq!(exit.block_entry_pc, 0x0);
        assert_eq!(exit.successor_pc, 0x8);
        assert_eq!(exit.exit_kind, super::ChainExitKind::UnconditionalBranch);
        assert_eq!(exit.patch_size, 5);
        assert!(!exit.patched);

        // The slot is in-bounds, starts at a `ret`, and original_bytes matches it.
        let bytes = blocks[0].lowered.bytes();
        assert!(exit.patch_offset + exit.patch_size <= bytes.len());
        assert_eq!(bytes[exit.patch_offset], 0xC3);
        assert_eq!(
            exit.original_bytes.as_slice(),
            &bytes[exit.patch_offset..exit.patch_offset + exit.patch_size]
        );

        // The halt block has no chain exit.
        assert!(blocks[1].lowered.chain_exits().is_empty());
    }

    #[test]
    fn lowers_one_function_block_by_guest_pc() {
        let function = two_block_branch_function();

        let block = lower_function_block(&function, 0x8)
            .expect("function should verify")
            .expect("block should exist");
        assert_eq!(block.entry_pc, 0x8);
        assert!(block.lowered.dump().contains("mov [rdi+0x110], rax"));

        assert!(
            lower_function_block(&function, 0xdead)
                .expect("function should verify")
                .is_none()
        );
    }

    #[test]
    fn lower_tiny_block_still_rejects_branches() {
        let mut function = two_block_branch_function();
        // Collapse to a single block whose branch targets itself: a valid CFG
        // (passes verification) with no sibling for the single-block path.
        function.blocks.truncate(1);
        function.blocks[0].terminator = Terminator::Branch { target: BlockId(0) };
        function.value_count = 1;
        let error = lower_tiny_block(&function).expect_err("branch must be rejected");
        assert!(matches!(
            error,
            super::LoweringError::UnsupportedTerminator {
                terminator: "branch"
            }
        ));
    }

    #[test]
    fn lowering_rejects_guards_for_now() {
        use nx86_ir::{Cond, DeoptId, DeoptPoint};

        // A verified two-block guard function: native guard emission is deferred
        // (it needs native flag materialization), so the lowerer reports it
        // unsupported, exactly like a conditional branch.
        let function = Function {
            name: "guarded".to_owned(),
            entry_address: 0,
            value_count: 0,
            deopt_points: vec![DeoptPoint {
                resume_pc: 0x100,
                reason: "guard:eq".to_owned(),
            }],
            blocks: vec![
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Guard {
                        cond: Cond::Eq,
                        if_pass: BlockId(1),
                        deopt: DeoptId(0),
                    },
                    terminator_address: 0,
                },
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Halt {
                        reason: "passed".to_owned(),
                    },
                    terminator_address: 0x4,
                },
            ],
        };

        let error = lower_function(&function).expect_err("guards are not lowered yet");
        assert!(matches!(
            error,
            super::LoweringError::UnsupportedTerminator {
                terminator: "guard"
            }
        ));
    }

    #[test]
    fn lowers_block_with_spills() {
        // Seven values live at once exceed the six-register pool, forcing one
        // spill slot (rounded up to 16 bytes).
        let lowered = lower_tiny_block(&spill_function()).expect("spill block should lower");

        assert_eq!(lowered.stack_size(), 16);
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

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    #[allow(unsafe_code)]
    fn calls_lowered_block_with_spills() {
        let lowered = lower_tiny_block(&spill_function()).expect("spill block should lower");
        let executable =
            nx86_jit::ExecutableMemory::new(lowered.bytes()).expect("code should allocate");
        let cpu = nx86_core::guest::CpuState::new();
        let mut state = NativeBlockState::from_cpu_state(&cpu);

        // SAFETY: `lower_tiny_block` produced an `extern "C"
        // fn(*mut NativeBlockState)` body for this exact state layout, exercising
        // the spill path for the seventh value.
        unsafe { executable.call_with_state(&mut state) }.expect("native block should run");
        let final_state = state.to_cpu_state(Some("svc #0x0"));

        for index in 0u32..7 {
            assert_eq!(final_state.x(index as u8), u64::from(index) + 1);
        }
        assert!(final_state.halted());
    }

    fn spill_function() -> Function {
        let mut instructions = Vec::new();
        for index in 0u32..7 {
            instructions.push(Inst {
                result: Some(Value(index)),
                op: Op::Const {
                    ty: Type::I64,
                    value: u64::from(index) + 1,
                },
                guest_address: 0,
            });
        }
        for index in 0u32..7 {
            instructions.push(Inst {
                result: None,
                op: Op::SetReg {
                    reg: Reg::X(index as u8),
                    value: Value(index),
                },
                guest_address: 0,
            });
        }
        Function {
            name: "spill".to_owned(),
            entry_address: 0,
            value_count: 7,
            deopt_points: Vec::new(),
            blocks: vec![Block {
                instructions,
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 0,
            }],
        }
    }

    fn tiny_add_function() -> Function {
        Function {
            name: "tiny_add".to_owned(),
            entry_address: 0,
            value_count: 4,
            deopt_points: Vec::new(),
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

    /// Two straight-line blocks: block 0 sets x0 = 5 then branches to block 1,
    /// which copies x0 into x1 and halts. Inter-block dataflow goes through the
    /// guest register file, so each block lowers independently.
    fn two_block_branch_function() -> Function {
        Function {
            name: "two_block".to_owned(),
            entry_address: 0,
            value_count: 2,
            deopt_points: Vec::new(),
            blocks: vec![
                Block {
                    instructions: vec![
                        Inst {
                            result: Some(Value(0)),
                            op: Op::Const {
                                ty: Type::I64,
                                value: 5,
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
                    ],
                    terminator: Terminator::Branch { target: BlockId(1) },
                    terminator_address: 4,
                },
                Block {
                    instructions: vec![
                        Inst {
                            result: Some(Value(1)),
                            op: Op::GetReg { reg: Reg::X(0) },
                            guest_address: 8,
                        },
                        Inst {
                            result: None,
                            op: Op::SetReg {
                                reg: Reg::X(1),
                                value: Value(1),
                            },
                            guest_address: 8,
                        },
                    ],
                    terminator: Terminator::Halt {
                        reason: "svc #0x0".to_owned(),
                    },
                    terminator_address: 8,
                },
            ],
        }
    }
}
