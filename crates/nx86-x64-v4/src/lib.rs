use std::collections::{BTreeMap, HashSet};

use nx86_core::guest::{CpuState, Nzcv};
use nx86_ir::verify::{self, VerifyError};
use nx86_ir::{
    BinaryOp, Block, BlockId, FpBinaryOp, Function, Inst, Op, Reg, Terminator, Type, Value,
    VectorBinaryOp, VectorCompareOp, VectorShuffle,
};
use nx86_profile::{ProfileEvent, ProfileLog};
use nx86_regalloc::{Allocation, Location, allocate};
use nx86_x64_asm::{
    AsmError, Assembler, CodeBuffer, Label, Mem64, PatchKind, Reg64, RegMask, RegXmm,
};
use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-x64-v4";

const X0_OFFSET: i32 = 0;
const SP_OFFSET: i32 = X0_OFFSET + 31 * 8;
const PC_OFFSET: i32 = SP_OFFSET + 8;
const NZCV_BITS_OFFSET: i32 = PC_OFFSET + 8;
const HALTED_OFFSET: i32 = NZCV_BITS_OFFSET + 8;
const FASTMEM_BASE_OFFSET: i32 = HALTED_OFFSET + 8;
const FASTMEM_PERMISSIONS_OFFSET: i32 = FASTMEM_BASE_OFFSET + 8;
const SLOWMEM_CONTEXT_OFFSET: i32 = FASTMEM_PERMISSIONS_OFFSET + 8;
const SLOWMEM_READ_OFFSET: i32 = SLOWMEM_CONTEXT_OFFSET + 8;
const SLOWMEM_WRITE_OFFSET: i32 = SLOWMEM_READ_OFFSET + 8;
const SLOWMEM_VALUE_OFFSET: i32 = SLOWMEM_WRITE_OFFSET + 8;
const MEMORY_FAULT_OFFSET: i32 = SLOWMEM_VALUE_OFFSET + 8;
const V0_OFFSET: i32 = MEMORY_FAULT_OFFSET + 8;
const SAVED_REGISTER_BYTES: u32 = 32;
const FASTMEM_READ: i32 = 1;
const FASTMEM_WRITE: i32 = 2;
const PAGE_MASK: i32 = 4095;
const PAGE_SHIFT: u8 = 12;
const ARENA_SIZE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const SHUFFLE_SWAP_D_IMM: u8 = 0x4e;

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
    /// Host base of the contiguous 64 GiB fastmem arena, or zero.
    pub fastmem_base: u64,
    /// Byte-per-page fastmem permission table, or zero.
    pub fastmem_permissions: u64,
    /// Opaque pointer consumed by the slowmem callbacks.
    pub slowmem_context: u64,
    /// `extern "C"` slowmem read callback address.
    pub slowmem_read: u64,
    /// `extern "C"` slowmem write callback address.
    pub slowmem_write: u64,
    /// Successful slowmem reads place their value here.
    pub slowmem_value: u64,
    /// Nonzero callback status asks the native block to side-exit.
    pub memory_fault: u64,
    /// FP/SIMD register file as low/high 64-bit lanes for v0..v31.
    pub v: [u64; 64],
    pub fpcr: u64,
    pub fpsr: u64,
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
            fastmem_base: 0,
            fastmem_permissions: 0,
            slowmem_context: 0,
            slowmem_read: 0,
            slowmem_write: 0,
            slowmem_value: 0,
            memory_fault: 0,
            v: vector_lanes_from_cpu(cpu),
            fpcr: u64::from(cpu.fpcr()),
            fpsr: u64::from(cpu.fpsr()),
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
        for index in 0..32 {
            let low = self.v[index * 2];
            let high = self.v[index * 2 + 1];
            cpu.set_vector(index as u8, u128::from(low) | (u128::from(high) << 64));
        }
        cpu.set_fpcr(self.fpcr as u32);
        cpu.set_fpsr(self.fpsr as u32);
        if self.halted != 0 {
            cpu.halt(halt_reason.unwrap_or("native block halted"));
        } else {
            cpu.clear_halt();
        }
        cpu
    }
}

fn vector_lanes_from_cpu(cpu: &CpuState) -> [u64; 64] {
    let mut lanes = [0u64; 64];
    for index in 0..32 {
        lanes[index * 2] = cpu.vector_lane64(index as u8, 0);
        lanes[index * 2 + 1] = cpu.vector_lane64(index as u8, 1);
    }
    lanes
}

#[derive(Debug, Error)]
pub enum LoweringError {
    #[error("input function failed NxIR verification: {0}")]
    InvalidIr(#[from] VerifyError),
    #[error("tiny native lowering supports exactly one block, got {count}")]
    UnsupportedBlockCount { count: usize },
    #[error("tiny native lowering does not support {op}")]
    UnsupportedOp { op: &'static str },
    #[error("tiny native lowering does not support guest memory type {ty:?}")]
    UnsupportedMemoryType { ty: Type },
    #[error("tiny native lowering does not support {terminator}")]
    UnsupportedTerminator { terminator: &'static str },
    #[error("branch target {target:?} is outside the function block table")]
    UnknownBranchTarget { target: BlockId },
    #[error("profiled layout selected unknown block entry {entry_pc:#x}")]
    UnknownLayoutEntry { entry_pc: u64 },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeSection {
    Hot,
    Cold,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockLayoutEntry {
    pub original_index: usize,
    pub entry_pc: u64,
    pub heat: u64,
    pub section: CodeSection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotColdLayout {
    entries: Vec<BlockLayoutEntry>,
}

impl HotColdLayout {
    #[must_use]
    pub fn entries(&self) -> &[BlockLayoutEntry] {
        &self.entries
    }
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

/// Lower a function in profile-guided hot/cold order. Blocks stay keyed by
/// guest PC, so dispatcher/native mappings remain stable while hot successors
/// appear earlier in the emitted native block list.
pub fn lower_function_with_profile(
    function: &Function,
    profile: &ProfileLog,
) -> Result<(Vec<LoweredFunctionBlock>, HotColdLayout), LoweringError> {
    verify::verify(function)?;
    if function.blocks.is_empty() {
        return Err(LoweringError::UnsupportedBlockCount { count: 0 });
    }

    let entry_pcs: Vec<u64> = function.blocks.iter().map(block_entry_pc).collect();
    let layout = hot_cold_layout(function, profile)?;
    let mut blocks = Vec::with_capacity(function.blocks.len());
    for entry in layout.entries() {
        let lowered = lower_block_with_entries(
            &function.blocks[entry.original_index],
            function.value_count,
            &entry_pcs,
        )?;
        blocks.push(LoweredFunctionBlock {
            entry_pc: entry.entry_pc,
            lowered,
        });
    }
    Ok((blocks, layout))
}

/// Compute the profile-guided hot/cold layout without emitting code. Entry block
/// remains first; the hottest observed successor chain follows; unobserved
/// blocks are split to the cold tail in original order.
pub fn hot_cold_layout(
    function: &Function,
    profile: &ProfileLog,
) -> Result<HotColdLayout, LoweringError> {
    verify::verify(function)?;
    let entry_pcs: Vec<u64> = function.blocks.iter().map(block_entry_pc).collect();
    if entry_pcs.is_empty() {
        return Err(LoweringError::UnsupportedBlockCount { count: 0 });
    }
    let mut heat = BTreeMap::<u64, u64>::new();
    let mut edges = BTreeMap::<(u64, u64), u64>::new();
    let known: HashSet<u64> = entry_pcs.iter().copied().collect();
    for record in &profile.records {
        match &record.event {
            ProfileEvent::JitBlock { guest_pc, .. } => {
                if known.contains(guest_pc) {
                    *heat.entry(*guest_pc).or_default() += 1;
                }
            }
            ProfileEvent::BranchTarget {
                source_pc,
                target_pc,
            } => {
                if known.contains(source_pc) {
                    *heat.entry(*source_pc).or_default() += 1;
                }
                if known.contains(target_pc) {
                    *heat.entry(*target_pc).or_default() += 1;
                }
                if known.contains(source_pc) && known.contains(target_pc) {
                    *edges.entry((*source_pc, *target_pc)).or_default() += 1;
                }
            }
            ProfileEvent::HelperCall { .. }
            | ProfileEvent::Slowmem { .. }
            | ProfileEvent::Fastmem { .. }
            | ProfileEvent::SmcInvalidate { .. } => {}
        }
    }

    let mut visited = HashSet::<u64>::new();
    let mut ordered = Vec::<u64>::new();
    let mut current = entry_pcs[0];
    loop {
        if !visited.insert(current) {
            break;
        }
        ordered.push(current);
        let next = edges
            .iter()
            .filter_map(|((source, target), count)| {
                (*source == current && !visited.contains(target) && *count > 0)
                    .then_some((*target, *count))
            })
            .max_by_key(|(target, count)| (*count, std::cmp::Reverse(*target)))
            .map(|(target, _)| target);
        let Some(next) = next else {
            break;
        };
        current = next;
    }

    let mut remaining_hot = entry_pcs
        .iter()
        .copied()
        .filter(|pc| !visited.contains(pc) && heat.get(pc).copied().unwrap_or(0) > 0)
        .collect::<Vec<_>>();
    remaining_hot.sort_by_key(|pc| (std::cmp::Reverse(heat.get(pc).copied().unwrap_or(0)), *pc));
    for pc in remaining_hot {
        visited.insert(pc);
        ordered.push(pc);
    }

    for pc in &entry_pcs {
        if visited.insert(*pc) {
            ordered.push(*pc);
        }
    }

    let mut entries = Vec::with_capacity(entry_pcs.len());
    for entry_pc in ordered {
        let Some(original_index) = entry_pcs.iter().position(|pc| *pc == entry_pc) else {
            return Err(LoweringError::UnknownLayoutEntry { entry_pc });
        };
        let heat = heat.get(&entry_pc).copied().unwrap_or(0);
        entries.push(BlockLayoutEntry {
            original_index,
            entry_pc,
            heat,
            section: if original_index == 0 || heat > 0 {
                CodeSection::Hot
            } else {
                CodeSection::Cold
            },
        });
    }
    Ok(HotColdLayout { entries })
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
    let memory_fault_exit = asm.create_label();
    asm.prologue();
    emit_native_register_prologue(&mut asm);
    if stack_size > 0 {
        asm.sub_reg_imm32(Reg64::Rsp, stack_size);
    }
    // A block that exits via a branch leaves the halted flag clear so the
    // dispatcher routes to the next guest PC; `Halt` sets it in the terminator.
    asm.mov_reg_imm64(Reg64::Rax, 0);
    asm.mov_mem_reg(state_mem(HALTED_OFFSET), Reg64::Rax);
    asm.mov_mem_reg(state_mem(MEMORY_FAULT_OFFSET), Reg64::Rax);

    for inst in &block.instructions {
        lower_inst(&mut asm, inst, &allocation, memory_fault_exit)?;
    }

    let chain_successor = emit_terminator(&mut asm, block, resolve_target)?;

    if stack_size > 0 {
        asm.add_reg_imm32(Reg64::Rsp, stack_size);
    }
    emit_native_register_epilogue(&mut asm);
    // An unconditional branch tears down its frame and ends in a patchable
    // chain-exit slot (so a hot edge can later jump straight to its successor);
    // every other terminator uses the plain `ret` epilogue. The slot must come
    // after both `add rsp` and `pop rbp` so a chained jump stays stack-balanced.
    match chain_successor {
        Some(_) => asm.chain_epilogue(),
        None => asm.epilogue(),
    }

    // Slowmem failures side-exit after restoring the complete native frame.
    // The backend reads the retained typed VMM error from the helper context.
    asm.bind_label(memory_fault_exit)?;
    if stack_size > 0 {
        asm.add_reg_imm32(Reg64::Rsp, stack_size);
    }
    emit_native_register_epilogue(&mut asm);
    asm.epilogue();

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

fn lower_inst(
    asm: &mut Assembler,
    inst: &Inst,
    alloc: &Allocation,
    memory_fault_exit: Label,
) -> Result<(), LoweringError> {
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
        Op::Trunc { value } | Op::ZeroExtend { value } => {
            let result = result_value(
                inst,
                if matches!(inst.op, Op::Trunc { .. }) {
                    "trunc"
                } else {
                    "zero-extend"
                },
            )?;
            materialize_value(asm, *value, alloc, Reg64::Rax)?;
            match location(alloc, result)? {
                Location::Register(dst) => asm.mov_reg_reg32(dst, Reg64::Rax),
                Location::Spill(slot) => {
                    asm.mov_reg_reg32(Reg64::Rax, Reg64::Rax);
                    asm.mov_mem_reg(spill_slot(slot)?, Reg64::Rax);
                }
            }
        }
        Op::Load { ty, address } => {
            let result = result_value(inst, "load")?;
            emit_guest_load(
                asm,
                *ty,
                *address,
                result,
                inst.guest_address,
                alloc,
                memory_fault_exit,
            )?;
        }
        Op::Store { ty, address, value } => {
            emit_guest_store(
                asm,
                *ty,
                *address,
                *value,
                inst.guest_address,
                alloc,
                memory_fault_exit,
            )?;
        }
        Op::SetFlags { .. } => {
            return Err(LoweringError::UnsupportedOp { op: "lazy flags" });
        }
        Op::LoadExclusive { .. } | Op::LoadAcquire { .. } => {
            return Err(LoweringError::UnsupportedOp { op: "atomic load" });
        }
        Op::StoreExclusive { .. } | Op::StoreRelease { .. } => {
            return Err(LoweringError::UnsupportedOp { op: "atomic store" });
        }
        Op::Barrier { .. } => {
            return Err(LoweringError::UnsupportedOp { op: "barrier" });
        }
        Op::FpMoveImmediate { rd, bits, .. } => {
            emit_store_vector_lane_imm(asm, *rd, 0, *bits)?;
            emit_store_vector_lane_imm(asm, *rd, 1, 0)?;
        }
        Op::FpScalarBinary { op, rd, rn, rm, .. } => {
            emit_scalar_fp_binary(asm, *op, *rd, *rn, *rm)?;
        }
        Op::FpCompare { .. } => {
            return Err(LoweringError::UnsupportedOp { op: "fp compare" });
        }
        Op::VectorBinary { op, rd, rn, rm, .. } => {
            emit_vector_binary(asm, *op, *rd, *rn, *rm)?;
        }
        Op::VectorCompare { op, rd, rn, rm, .. } => {
            emit_vector_compare(asm, *op, *rd, *rn, *rm)?;
        }
        Op::VectorShuffle {
            shuffle, rd, rn, ..
        } => {
            emit_vector_shuffle(asm, *shuffle, *rd, *rn)?;
        }
    }

    Ok(())
}

fn emit_store_vector_lane_imm(
    asm: &mut Assembler,
    register: u8,
    lane: u8,
    value: u64,
) -> Result<(), LoweringError> {
    asm.mov_reg_imm64(Reg64::Rax, value);
    asm.mov_mem_reg(vector_lane_mem(register, lane)?, Reg64::Rax);
    Ok(())
}

fn emit_scalar_fp_binary(
    asm: &mut Assembler,
    op: FpBinaryOp,
    rd: u8,
    rn: u8,
    rm: u8,
) -> Result<(), LoweringError> {
    asm.movsd_xmm_mem(RegXmm::Xmm0, vector_lane_mem(rn, 0)?);
    match op {
        FpBinaryOp::Add => asm.addsd_xmm_mem(RegXmm::Xmm0, vector_lane_mem(rm, 0)?),
        FpBinaryOp::Sub => asm.subsd_xmm_mem(RegXmm::Xmm0, vector_lane_mem(rm, 0)?),
        FpBinaryOp::Mul => asm.mulsd_xmm_mem(RegXmm::Xmm0, vector_lane_mem(rm, 0)?),
        FpBinaryOp::Div => asm.divsd_xmm_mem(RegXmm::Xmm0, vector_lane_mem(rm, 0)?),
    }
    asm.movsd_mem_xmm(vector_lane_mem(rd, 0)?, RegXmm::Xmm0);
    asm.mov_reg_imm64(Reg64::Rax, 0);
    asm.mov_mem_reg(vector_lane_mem(rd, 1)?, Reg64::Rax);
    Ok(())
}

fn emit_vector_binary(
    asm: &mut Assembler,
    op: VectorBinaryOp,
    rd: u8,
    rn: u8,
    rm: u8,
) -> Result<(), LoweringError> {
    asm.movdqu_xmm_mem(RegXmm::Xmm0, vector_mem(rn)?);
    match op {
        VectorBinaryOp::AddI64 => {
            asm.paddq_xmm_mem(RegXmm::Xmm0, vector_mem(rm)?);
        }
        VectorBinaryOp::AddF64 => {
            asm.addpd_xmm_mem(RegXmm::Xmm0, vector_mem(rm)?);
        }
    }
    asm.movdqu_mem_xmm(vector_mem(rd)?, RegXmm::Xmm0);
    Ok(())
}

fn emit_vector_compare(
    asm: &mut Assembler,
    op: VectorCompareOp,
    rd: u8,
    rn: u8,
    rm: u8,
) -> Result<(), LoweringError> {
    asm.vmovdqu64_xmm_mem(RegXmm::Xmm0, vector_mem(rn)?);
    asm.vmovdqu64_xmm_mem(RegXmm::Xmm1, vector_mem(rm)?);
    match op {
        VectorCompareOp::EqI64 => {
            asm.vpcmpeqq_mask_xmm_xmm(RegMask::K1, RegXmm::Xmm0, RegXmm::Xmm1);
            asm.vpmovm2q_xmm_mask(RegXmm::Xmm0, RegMask::K1);
        }
    }
    asm.vmovdqu64_mem_xmm(vector_mem(rd)?, RegXmm::Xmm0);
    Ok(())
}

fn emit_vector_shuffle(
    asm: &mut Assembler,
    shuffle: VectorShuffle,
    rd: u8,
    rn: u8,
) -> Result<(), LoweringError> {
    match shuffle {
        VectorShuffle::SwapD => {
            asm.pshufd_xmm_mem_imm8(RegXmm::Xmm0, vector_mem(rn)?, SHUFFLE_SWAP_D_IMM);
        }
    }
    asm.movdqu_mem_xmm(vector_mem(rd)?, RegXmm::Xmm0);
    Ok(())
}

fn emit_guest_load(
    asm: &mut Assembler,
    ty: Type,
    address: Value,
    result: Value,
    guest_address: u64,
    alloc: &Allocation,
    memory_fault_exit: Label,
) -> Result<(), LoweringError> {
    let size = memory_size(ty)?;
    let slow = asm.create_label();
    let done = asm.create_label();
    materialize_value(asm, address, alloc, Reg64::Rax)?;
    emit_fastmem_check(asm, size, FASTMEM_READ, slow)?;
    match ty {
        Type::I32 => asm.mov_reg_mem32(Reg64::Rax, Mem64::indexed(Reg64::R14, Reg64::Rax, 0)),
        Type::I64 => asm.mov_reg_mem(Reg64::Rax, Mem64::indexed(Reg64::R14, Reg64::Rax, 0)),
        other => return Err(LoweringError::UnsupportedMemoryType { ty: other }),
    }
    store_into_value(asm, result, alloc, Reg64::Rax)?;
    asm.jmp(done)?;

    asm.bind_label(slow)?;
    emit_slowmem_call(
        asm,
        SLOWMEM_READ_OFFSET,
        size,
        guest_address,
        None,
        memory_fault_exit,
    )?;
    asm.mov_reg_mem(Reg64::Rax, state_mem(SLOWMEM_VALUE_OFFSET));
    if ty == Type::I32 {
        asm.mov_reg_reg32(Reg64::Rax, Reg64::Rax);
    }
    store_into_value(asm, result, alloc, Reg64::Rax)?;
    asm.bind_label(done)?;
    Ok(())
}

fn emit_guest_store(
    asm: &mut Assembler,
    ty: Type,
    address: Value,
    value: Value,
    guest_address: u64,
    alloc: &Allocation,
    memory_fault_exit: Label,
) -> Result<(), LoweringError> {
    let size = memory_size(ty)?;
    let slow = asm.create_label();
    let done = asm.create_label();
    materialize_value(asm, address, alloc, Reg64::Rax)?;
    emit_fastmem_check(asm, size, FASTMEM_WRITE, slow)?;
    materialize_value(asm, value, alloc, Reg64::Rcx)?;
    match ty {
        Type::I32 => asm.mov_mem_reg32(Mem64::indexed(Reg64::R14, Reg64::Rax, 0), Reg64::Rcx),
        Type::I64 => asm.mov_mem_reg(Mem64::indexed(Reg64::R14, Reg64::Rax, 0), Reg64::Rcx),
        other => return Err(LoweringError::UnsupportedMemoryType { ty: other }),
    }
    asm.jmp(done)?;

    asm.bind_label(slow)?;
    materialize_value(asm, value, alloc, Reg64::Rcx)?;
    emit_slowmem_call(
        asm,
        SLOWMEM_WRITE_OFFSET,
        size,
        guest_address,
        Some(Reg64::Rcx),
        memory_fault_exit,
    )?;
    asm.bind_label(done)?;
    Ok(())
}

fn memory_size(ty: Type) -> Result<u64, LoweringError> {
    match ty {
        Type::I32 => Ok(4),
        Type::I64 => Ok(8),
        other => Err(LoweringError::UnsupportedMemoryType { ty: other }),
    }
}

/// Branch to `slow` unless RAX names one eligible, in-arena, single-page access.
fn emit_fastmem_check(
    asm: &mut Assembler,
    size: u64,
    permission: i32,
    slow: Label,
) -> Result<(), LoweringError> {
    asm.cmp_reg_imm32(Reg64::R14, 0);
    asm.jz(slow)?;
    asm.cmp_reg_imm32(Reg64::R13, 0);
    asm.jz(slow)?;
    asm.mov_reg_imm64(Reg64::Rcx, ARENA_SIZE_BYTES - size);
    asm.cmp_reg_reg(Reg64::Rax, Reg64::Rcx);
    asm.ja(slow)?;
    asm.mov_reg_reg(Reg64::Rcx, Reg64::Rax);
    asm.and_reg_imm32(Reg64::Rcx, PAGE_MASK);
    asm.cmp_reg_imm32(Reg64::Rcx, i32::try_from(4096 - size).unwrap_or(0));
    asm.ja(slow)?;
    asm.mov_reg_reg(Reg64::Rcx, Reg64::Rax);
    asm.shr_reg_imm8(Reg64::Rcx, PAGE_SHIFT);
    asm.movzx_reg_mem8(Reg64::Rcx, Mem64::indexed(Reg64::R13, Reg64::Rcx, 0));
    asm.test_reg_imm32(Reg64::Rcx, permission);
    asm.jz(slow)?;
    Ok(())
}

fn emit_slowmem_call(
    asm: &mut Assembler,
    callback_offset: i32,
    size: u64,
    guest_address: u64,
    value: Option<Reg64>,
    memory_fault_exit: Label,
) -> Result<(), LoweringError> {
    // Preserve every allocator and scratch register across the Rust ABI call.
    for reg in [
        Reg64::Rax,
        Reg64::Rcx,
        Reg64::Rdx,
        Reg64::Rsi,
        Reg64::R8,
        Reg64::R9,
        Reg64::R10,
        Reg64::R11,
    ] {
        asm.push_reg(reg);
    }
    asm.mov_reg_reg(Reg64::Rdx, Reg64::Rax);
    if value.is_none() {
        asm.mov_reg_imm64(Reg64::Rcx, 0);
    }
    asm.mov_reg_imm64(Reg64::R8, size);
    asm.mov_reg_reg(Reg64::Rdi, Reg64::R12);
    asm.mov_reg_reg(Reg64::Rsi, Reg64::R15);
    asm.mov_reg_imm64(Reg64::R9, guest_address);
    asm.mov_reg_mem(Reg64::Rax, state_mem(callback_offset));
    asm.call_reg(Reg64::Rax);
    asm.mov_mem_reg(state_mem(MEMORY_FAULT_OFFSET), Reg64::Rax);
    for reg in [
        Reg64::R11,
        Reg64::R10,
        Reg64::R9,
        Reg64::R8,
        Reg64::Rsi,
        Reg64::Rdx,
        Reg64::Rcx,
        Reg64::Rax,
    ] {
        asm.pop_reg(reg);
    }
    asm.mov_reg_mem(Reg64::Rax, state_mem(MEMORY_FAULT_OFFSET));
    asm.cmp_reg_imm32(Reg64::Rax, 0);
    asm.jnz(memory_fault_exit)?;
    Ok(())
}

fn emit_native_register_prologue(asm: &mut Assembler) {
    for reg in [Reg64::R12, Reg64::R13, Reg64::R14, Reg64::R15] {
        asm.push_reg(reg);
    }
    asm.mov_reg_reg(Reg64::R15, Reg64::Rdi);
    asm.mov_reg_mem(Reg64::R14, state_mem(FASTMEM_BASE_OFFSET));
    asm.mov_reg_mem(Reg64::R13, state_mem(FASTMEM_PERMISSIONS_OFFSET));
    asm.mov_reg_mem(Reg64::R12, state_mem(SLOWMEM_CONTEXT_OFFSET));
}

fn emit_native_register_epilogue(asm: &mut Assembler) {
    for reg in [Reg64::R15, Reg64::R14, Reg64::R13, Reg64::R12] {
        asm.pop_reg(reg);
    }
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
    let offset = i32::try_from(u64::from(SAVED_REGISTER_BYTES) + (u64::from(slot) + 1) * 8)
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

fn vector_lane_mem(register: u8, lane: u8) -> Result<Mem64, LoweringError> {
    if register >= 32 {
        return Err(LoweringError::RegisterOutOfRange { register });
    }
    let lane_offset = match lane {
        0 => 0,
        1 => 8,
        _ => return Err(LoweringError::UnsupportedOp { op: "vector lane" }),
    };
    Ok(state_mem(
        V0_OFFSET + (i32::from(register) * 16) + lane_offset,
    ))
}

fn vector_mem(register: u8) -> Result<Mem64, LoweringError> {
    if register >= 32 {
        return Err(LoweringError::RegisterOutOfRange { register });
    }
    Ok(state_mem(V0_OFFSET + (i32::from(register) * 16)))
}

const fn state_mem(offset: i32) -> Mem64 {
    Mem64::new(Reg64::R15, offset)
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

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
        assert!(lowered.dump().contains("mov [r15+0x100], rax"));
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
    fn lowers_guest_memory_with_fast_and_slow_paths() {
        let function = tiny_memory_function();

        let lowered = lower_tiny_block(&function).expect("guest memory should lower");
        let dump = lowered.dump();

        assert!(dump.contains("mov [r14+rax], rcx"));
        assert!(dump.contains("mov rax, [r14+rax]"));
        assert!(dump.contains("movzx rcx, byte [r13+rcx]"));
        assert!(dump.contains("call rax"));
    }

    #[test]
    fn lowers_scalar_fp_and_vector_ops() {
        let function = fp_vector_function(false);

        let lowered = lower_tiny_block(&function).expect("fp/vector ops should lower");
        let dump = lowered.dump();

        assert!(dump.contains("movsd xmm0"));
        assert!(dump.contains("addsd xmm0"));
        assert!(dump.contains("paddq xmm0"));
        assert!(dump.contains("addpd xmm0"));
        assert!(dump.contains("mov [r15+0x"));
    }

    #[test]
    fn lowers_advanced_vector_ops_with_masks_and_shuffle() {
        let function = advanced_vector_function();

        let lowered = lower_tiny_block(&function).expect("advanced vector ops should lower");
        let dump = lowered.dump();

        assert!(dump.contains("vmovdqu64 xmm0"));
        assert!(dump.contains("vpcmpeqq k1"));
        assert!(dump.contains("vpmovm2q xmm0"));
        assert!(dump.contains("pshufd xmm0"));
        assert!(dump.contains("vmovdqu64 xmmword [r15+0x"));
    }

    #[test]
    fn lowering_rejects_fp_compare_for_now() {
        let function = fp_vector_function(true);

        let error = lower_tiny_block(&function).expect_err("fp compare is deferred");

        assert!(matches!(
            error,
            super::LoweringError::UnsupportedOp { op: "fp compare" }
        ));
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
        assert!(branch_dump.contains("mov [r15+0x100], rax"));
        assert!(!branch_dump.contains("mov rax, 0x1"));

        // Block 1 halts: it sets the halted flag (offset 0x110) to 1.
        let halt_dump = blocks[1].lowered.dump();
        assert!(halt_dump.contains("mov rax, 0x1"));
        assert!(halt_dump.contains("mov [r15+0x110], rax"));
    }

    #[test]
    fn profiled_layout_moves_hot_successor_before_cold_block() {
        let function = hot_cold_function();
        let profile = nx86_profile::ProfileLog {
            records: vec![
                nx86_profile::ProfileRecord::new(nx86_profile::ProfileEvent::BranchTarget {
                    source_pc: 0x0,
                    target_pc: 0x10,
                }),
                nx86_profile::ProfileRecord::new(nx86_profile::ProfileEvent::JitBlock {
                    guest_pc: 0x10,
                    code_size_bytes: 32,
                    cache_file_name: "0000000000000010.nxo".to_owned(),
                }),
            ],
            recovered_truncated_tail: false,
        };

        let (blocks, layout) =
            super::lower_function_with_profile(&function, &profile).expect("profiled lower");
        let order = layout
            .entries()
            .iter()
            .map(|entry| (entry.entry_pc, entry.section))
            .collect::<Vec<_>>();

        assert_eq!(
            order,
            vec![
                (0x0, super::CodeSection::Hot),
                (0x10, super::CodeSection::Hot),
                (0x8, super::CodeSection::Cold),
            ]
        );
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.entry_pc)
                .collect::<Vec<_>>(),
            vec![0x0, 0x10, 0x8]
        );
        assert!(blocks[0].lowered.dump().contains("mov rax, 0x10"));
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
        assert!(block.lowered.dump().contains("mov [r15+0x110], rax"));

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
    fn native_memory_abi_offsets_are_stable() {
        assert_eq!(std::mem::offset_of!(NativeBlockState, x), 0);
        assert_eq!(std::mem::offset_of!(NativeBlockState, sp), 248);
        assert_eq!(std::mem::offset_of!(NativeBlockState, pc), 256);
        assert_eq!(std::mem::offset_of!(NativeBlockState, halted), 272);
        assert_eq!(std::mem::offset_of!(NativeBlockState, fastmem_base), 280);
        assert_eq!(
            std::mem::offset_of!(NativeBlockState, fastmem_permissions),
            288
        );
        assert_eq!(std::mem::offset_of!(NativeBlockState, slowmem_context), 296);
        assert_eq!(std::mem::offset_of!(NativeBlockState, slowmem_read), 304);
        assert_eq!(std::mem::offset_of!(NativeBlockState, slowmem_write), 312);
        assert_eq!(std::mem::offset_of!(NativeBlockState, slowmem_value), 320);
        assert_eq!(std::mem::offset_of!(NativeBlockState, memory_fault), 328);
        assert_eq!(std::mem::offset_of!(NativeBlockState, v), 336);
        assert_eq!(std::mem::offset_of!(NativeBlockState, fpcr), 848);
        assert_eq!(std::mem::offset_of!(NativeBlockState, fpsr), 856);
        assert_eq!(size_of::<NativeBlockState>(), 864);
    }

    #[test]
    fn vector_lane_addressing_rejects_invalid_lane() {
        let error = super::vector_lane_mem(0, 2).expect_err("lane 2 should be invalid");

        assert!(matches!(
            error,
            super::LoweringError::UnsupportedOp { op: "vector lane" }
        ));
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

    fn fp_vector_function(include_compare: bool) -> Function {
        let mut instructions = vec![
            Inst {
                result: None,
                op: Op::FpMoveImmediate {
                    precision: nx86_ir::FpPrecision::F64,
                    rd: 0,
                    bits: 1.0f64.to_bits(),
                },
                guest_address: 0,
            },
            Inst {
                result: None,
                op: Op::FpMoveImmediate {
                    precision: nx86_ir::FpPrecision::F64,
                    rd: 1,
                    bits: 2.0f64.to_bits(),
                },
                guest_address: 4,
            },
            Inst {
                result: None,
                op: Op::FpScalarBinary {
                    op: nx86_ir::FpBinaryOp::Add,
                    precision: nx86_ir::FpPrecision::F64,
                    rd: 2,
                    rn: 0,
                    rm: 1,
                },
                guest_address: 8,
            },
            Inst {
                result: None,
                op: Op::VectorBinary {
                    op: nx86_ir::VectorBinaryOp::AddF64,
                    arrangement: nx86_ir::VectorArrangement::TwoD,
                    rd: 3,
                    rn: 0,
                    rm: 1,
                },
                guest_address: 12,
            },
            Inst {
                result: None,
                op: Op::VectorBinary {
                    op: nx86_ir::VectorBinaryOp::AddI64,
                    arrangement: nx86_ir::VectorArrangement::TwoD,
                    rd: 4,
                    rn: 0,
                    rm: 1,
                },
                guest_address: 16,
            },
        ];
        if include_compare {
            instructions.push(Inst {
                result: None,
                op: Op::FpCompare {
                    precision: nx86_ir::FpPrecision::F64,
                    rn: 0,
                    rm: 1,
                },
                guest_address: 20,
            });
        }
        Function {
            name: "fp_vector".to_owned(),
            entry_address: 0,
            value_count: 0,
            deopt_points: Vec::new(),
            blocks: vec![Block {
                instructions,
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 24,
            }],
        }
    }

    fn advanced_vector_function() -> Function {
        Function {
            name: "advanced_vector".to_owned(),
            entry_address: 0,
            value_count: 0,
            deopt_points: Vec::new(),
            blocks: vec![Block {
                instructions: vec![
                    Inst {
                        result: None,
                        op: Op::FpMoveImmediate {
                            precision: nx86_ir::FpPrecision::F64,
                            rd: 0,
                            bits: 1.0f64.to_bits(),
                        },
                        guest_address: 0,
                    },
                    Inst {
                        result: None,
                        op: Op::FpMoveImmediate {
                            precision: nx86_ir::FpPrecision::F64,
                            rd: 1,
                            bits: 1.0f64.to_bits(),
                        },
                        guest_address: 4,
                    },
                    Inst {
                        result: None,
                        op: Op::VectorCompare {
                            op: nx86_ir::VectorCompareOp::EqI64,
                            arrangement: nx86_ir::VectorArrangement::TwoD,
                            rd: 2,
                            rn: 0,
                            rm: 1,
                        },
                        guest_address: 8,
                    },
                    Inst {
                        result: None,
                        op: Op::VectorShuffle {
                            shuffle: nx86_ir::VectorShuffle::SwapD,
                            arrangement: nx86_ir::VectorArrangement::TwoD,
                            rd: 3,
                            rn: 2,
                        },
                        guest_address: 12,
                    },
                ],
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 16,
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

    fn tiny_memory_function() -> Function {
        Function {
            name: "tiny_memory".to_owned(),
            entry_address: 0,
            value_count: 3,
            deopt_points: Vec::new(),
            blocks: vec![Block {
                instructions: vec![
                    Inst {
                        result: Some(Value(0)),
                        op: Op::Const {
                            ty: Type::I64,
                            value: 0x1000,
                        },
                        guest_address: 0,
                    },
                    Inst {
                        result: Some(Value(1)),
                        op: Op::Const {
                            ty: Type::I64,
                            value: 0x1122_3344_5566_7788,
                        },
                        guest_address: 4,
                    },
                    Inst {
                        result: None,
                        op: Op::Store {
                            ty: Type::I64,
                            address: Value(0),
                            value: Value(1),
                        },
                        guest_address: 8,
                    },
                    Inst {
                        result: Some(Value(2)),
                        op: Op::Load {
                            ty: Type::I64,
                            address: Value(0),
                        },
                        guest_address: 12,
                    },
                    Inst {
                        result: None,
                        op: Op::SetReg {
                            reg: Reg::X(0),
                            value: Value(2),
                        },
                        guest_address: 12,
                    },
                ],
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 16,
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

    fn hot_cold_function() -> Function {
        Function {
            name: "hot_cold".to_owned(),
            entry_address: 0,
            value_count: 3,
            deopt_points: Vec::new(),
            blocks: vec![
                Block {
                    instructions: vec![Inst {
                        result: Some(Value(0)),
                        op: Op::Const {
                            ty: Type::I64,
                            value: 7,
                        },
                        guest_address: 0,
                    }],
                    terminator: Terminator::Branch { target: BlockId(2) },
                    terminator_address: 4,
                },
                Block {
                    instructions: vec![
                        Inst {
                            result: Some(Value(1)),
                            op: Op::Const {
                                ty: Type::I64,
                                value: 1,
                            },
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
                        reason: "cold".to_owned(),
                    },
                    terminator_address: 8,
                },
                Block {
                    instructions: vec![
                        Inst {
                            result: Some(Value(2)),
                            op: Op::Const {
                                ty: Type::I64,
                                value: 2,
                            },
                            guest_address: 0x10,
                        },
                        Inst {
                            result: None,
                            op: Op::SetReg {
                                reg: Reg::X(0),
                                value: Value(2),
                            },
                            guest_address: 0x10,
                        },
                    ],
                    terminator: Terminator::Halt {
                        reason: "hot".to_owned(),
                    },
                    terminator_address: 0x10,
                },
            ],
        }
    }
}
