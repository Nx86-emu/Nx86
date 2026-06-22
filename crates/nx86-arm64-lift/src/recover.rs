//! Recursive-descent CFG recovery (Phase 26).
//!
//! Unlike [`lift_program`](crate::lift_program), which builds a basic-block CFG
//! over a contiguous, pre-decoded instruction slice for a *known* function,
//! recovery *derives* structure: starting from one or more entry PCs it decodes
//! on demand, follows direct branch and fall-through successors through a
//! worklist, and reports the reachable blocks, the per-function block sets, and
//! any unresolved exits.
//!
//! This is pure analysis — no native code generation — so it runs and tests on
//! every host. v0 follows only direct branches (`B`/`B.cond`) and the synthetic
//! `SVC` exit; indirect or out-of-range successors are recorded as
//! [`EdgeKind::Unresolved`] and not followed. `BL`/call discovery (multiple
//! functions from one image) arrives with broader decoder coverage; the seam is
//! marked in `explore`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use nx86_arm64_decode::{InstructionKind, decode_instruction};
use thiserror::Error;

/// Width of one AArch64 instruction in bytes.
const INSTRUCTION_BYTES: u64 = 4;

/// A read-only window over a contiguous block of guest code at a fixed base.
///
/// Recovery reads one 4-byte word at a time through `CodeView::word_bytes`,
/// so addresses outside the window (or misaligned) decode as
/// [`EdgeKind::Unresolved`] rather than panicking.
#[derive(Clone, Copy, Debug)]
pub struct CodeView<'a> {
    base_address: u64,
    bytes: &'a [u8],
}

impl<'a> CodeView<'a> {
    /// Wrap `bytes` as guest code beginning at `base_address`. The length must
    /// be a whole number of 4-byte instructions.
    pub fn new(base_address: u64, bytes: &'a [u8]) -> Result<Self, RecoverError> {
        if !bytes.len().is_multiple_of(INSTRUCTION_BYTES as usize) {
            return Err(RecoverError::UnalignedCode { len: bytes.len() });
        }
        Ok(Self {
            base_address,
            bytes,
        })
    }

    /// The four raw bytes of the instruction at `address`, if that address is a
    /// 4-byte-aligned instruction fully inside this window.
    fn word_bytes(&self, address: u64) -> Option<[u8; 4]> {
        let offset = address.checked_sub(self.base_address)?;
        if !offset.is_multiple_of(INSTRUCTION_BYTES) {
            return None;
        }
        let offset = usize::try_from(offset).ok()?;
        let end = offset.checked_add(INSTRUCTION_BYTES as usize)?;
        let slice = self.bytes.get(offset..end)?;
        Some([slice[0], slice[1], slice[2], slice[3]])
    }
}

/// The control-flow exit of a recovered block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// The block runs off its end into the next block (a leader boundary).
    Fallthrough,
    /// Unconditional direct branch (`B`).
    DirectBranch,
    /// Conditional branch (`B.cond`): the taken target and the fall-through.
    CondBranch { taken: u64, not_taken: u64 },
    /// Synthetic `SVC` program exit.
    Exit,
    /// Indirect, undecodable, or out-of-range successor: recorded, not followed.
    Unresolved,
}

/// One recovered basic block, keyed in the table by its `start` address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredBlock {
    /// Guest address of the first instruction.
    pub start: u64,
    /// Guest address one past the last instruction (exclusive).
    pub end: u64,
    /// Number of instructions in the block.
    pub instruction_count: usize,
    /// How the block transfers control.
    pub terminator: EdgeKind,
    /// Resolved (in-range, decoded) successor block starts, in edge order.
    pub successors: Vec<u64>,
}

/// A function candidate: an entry PC and the block starts reachable from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredFunction {
    /// Guest address the function is entered at.
    pub entry: u64,
    /// Sorted starts of every block reachable from `entry`.
    pub block_starts: Vec<u64>,
}

/// A recovered control-flow graph: function candidates plus the block table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredCfg {
    /// One candidate per distinct entry PC, in the order entries were supplied.
    pub functions: Vec<RecoveredFunction>,
    /// Every recovered block, keyed by start address for a deterministic table.
    pub blocks: BTreeMap<u64, RecoveredBlock>,
}

/// A failure that prevents recovery from starting.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RecoverError {
    #[error("code length {len} is not a multiple of 4 bytes")]
    UnalignedCode { len: usize },
    #[error("no entry points supplied")]
    NoEntries,
    #[error("entry {entry:#x} is not a decodable instruction in range")]
    UndecodableEntry { entry: u64 },
}

/// Instruction successor recorded during exploration.
#[derive(Clone, Copy, Debug)]
enum Succ {
    Exit,
    Direct(u64),
    Cond { taken: u64, not_taken: u64 },
    Fallthrough(u64),
}

/// Recover the control-flow graph reachable from `entries` within `code`.
///
/// Every supplied entry must be a decodable instruction inside `code`;
/// successors that leave the window or fail to decode are reported as
/// [`EdgeKind::Unresolved`] instead of failing the whole pass.
pub fn recover_cfg(code: &CodeView<'_>, entries: &[u64]) -> Result<RecoveredCfg, RecoverError> {
    if entries.is_empty() {
        return Err(RecoverError::NoEntries);
    }
    for &entry in entries {
        let decodable = code
            .word_bytes(entry)
            .is_some_and(|raw| decode_instruction(raw, entry).is_ok());
        if !decodable {
            return Err(RecoverError::UndecodableEntry { entry });
        }
    }

    let (decoded, leaders) = explore(code, entries);
    let blocks = build_blocks(&decoded, &leaders);
    let functions = discover_functions(entries, &blocks);
    Ok(RecoveredCfg { functions, blocks })
}

/// Phase A: recursive descent. Decode reachable instructions and collect the
/// set of block leaders (entries, branch targets, and conditional fall-throughs).
fn explore(code: &CodeView<'_>, entries: &[u64]) -> (BTreeMap<u64, Succ>, BTreeSet<u64>) {
    let mut decoded: BTreeMap<u64, Succ> = BTreeMap::new();
    let mut leaders: BTreeSet<u64> = entries.iter().copied().collect();
    let mut work: Vec<u64> = entries.to_vec();

    while let Some(start) = work.pop() {
        let mut addr = start;
        loop {
            if decoded.contains_key(&addr) {
                // A fall-through (or re-entry) into already-explored code; such
                // an address is always a leader, so the predecessor's edge is
                // resolved during block construction.
                break;
            }
            let Some(raw) = code.word_bytes(addr) else {
                break;
            };
            let kind = match decode_instruction(raw, addr) {
                Ok(instruction) => instruction.kind,
                Err(_) => break,
            };
            let next = addr.checked_add(INSTRUCTION_BYTES);
            match kind {
                InstructionKind::Svc { .. } => {
                    decoded.insert(addr, Succ::Exit);
                    break;
                }
                // `BL` is not decoded yet; once it is, its target becomes a new
                // function candidate enqueued here rather than a local leader.
                InstructionKind::Branch { target, .. } => {
                    leaders.insert(target);
                    work.push(target);
                    decoded.insert(addr, Succ::Direct(target));
                    break;
                }
                InstructionKind::CondBranch { target, .. } => {
                    leaders.insert(target);
                    work.push(target);
                    match next {
                        Some(not_taken) => {
                            leaders.insert(not_taken);
                            work.push(not_taken);
                            decoded.insert(
                                addr,
                                Succ::Cond {
                                    taken: target,
                                    not_taken,
                                },
                            );
                        }
                        None => {
                            decoded.insert(addr, Succ::Direct(target));
                        }
                    }
                    break;
                }
                _ => match next {
                    Some(next_addr) => {
                        decoded.insert(addr, Succ::Fallthrough(next_addr));
                        addr = next_addr;
                    }
                    None => {
                        decoded.insert(
                            addr,
                            Succ::Fallthrough(addr.wrapping_add(INSTRUCTION_BYTES)),
                        );
                        break;
                    }
                },
            }
        }
    }

    (decoded, leaders)
}

/// Phase B: group decoded instructions into blocks, splitting at every leader.
fn build_blocks(
    decoded: &BTreeMap<u64, Succ>,
    leaders: &BTreeSet<u64>,
) -> BTreeMap<u64, RecoveredBlock> {
    let mut blocks = BTreeMap::new();
    for &start in leaders {
        if !decoded.contains_key(&start) {
            // A leader that was never decoded (out-of-range branch target) is an
            // unresolved sink, not a block of its own.
            continue;
        }
        let mut addr = start;
        let mut instruction_count = 0usize;
        let (terminator, successors, last_addr) = loop {
            let Some(&succ) = decoded.get(&addr) else {
                break (EdgeKind::Unresolved, Vec::new(), addr);
            };
            instruction_count += 1;
            match succ {
                Succ::Exit => break (EdgeKind::Exit, Vec::new(), addr),
                Succ::Direct(target) => {
                    if decoded.contains_key(&target) {
                        break (EdgeKind::DirectBranch, vec![target], addr);
                    }
                    break (EdgeKind::Unresolved, Vec::new(), addr);
                }
                Succ::Cond { taken, not_taken } => {
                    let mut successors = Vec::new();
                    if decoded.contains_key(&taken) {
                        successors.push(taken);
                    }
                    // A `B.cond` to its own fall-through (`taken == not_taken`)
                    // has a single edge; do not record it twice.
                    if not_taken != taken && decoded.contains_key(&not_taken) {
                        successors.push(not_taken);
                    }
                    break (EdgeKind::CondBranch { taken, not_taken }, successors, addr);
                }
                Succ::Fallthrough(next) => {
                    if leaders.contains(&next) {
                        if decoded.contains_key(&next) {
                            break (EdgeKind::Fallthrough, vec![next], addr);
                        }
                        break (EdgeKind::Unresolved, Vec::new(), addr);
                    }
                    if !decoded.contains_key(&next) {
                        break (EdgeKind::Unresolved, Vec::new(), addr);
                    }
                    addr = next;
                }
            }
        };
        let end = last_addr
            .checked_add(INSTRUCTION_BYTES)
            .unwrap_or(last_addr);
        blocks.insert(
            start,
            RecoveredBlock {
                start,
                end,
                instruction_count,
                terminator,
                successors,
            },
        );
    }
    blocks
}

/// Phase C: each distinct entry seeds one function candidate covering the blocks
/// transitively reachable from it.
fn discover_functions(
    entries: &[u64],
    blocks: &BTreeMap<u64, RecoveredBlock>,
) -> Vec<RecoveredFunction> {
    let mut functions = Vec::new();
    let mut seen_entries = BTreeSet::new();
    for &entry in entries {
        if !seen_entries.insert(entry) {
            continue;
        }
        let mut reachable = BTreeSet::new();
        let mut stack = vec![entry];
        while let Some(addr) = stack.pop() {
            if !reachable.insert(addr) {
                continue;
            }
            if let Some(block) = blocks.get(&addr) {
                stack.extend(block.successors.iter().copied());
            }
        }
        // Keep only addresses that are real block starts (an entry with no
        // decoded block yields an empty candidate).
        reachable.retain(|addr| blocks.contains_key(addr));
        functions.push(RecoveredFunction {
            entry,
            block_starts: reachable.into_iter().collect(),
        });
    }
    functions
}

impl EdgeKind {
    fn render(&self, f: &mut fmt::Formatter<'_>, successors: &[u64]) -> fmt::Result {
        match self {
            Self::Exit => write!(f, "exit"),
            Self::Unresolved => write!(f, "unresolved"),
            Self::DirectBranch => render_targets(f, "branch", successors),
            Self::Fallthrough => render_targets(f, "fallthrough", successors),
            Self::CondBranch { .. } => render_targets(f, "cond", successors),
        }
    }
}

fn render_targets(f: &mut fmt::Formatter<'_>, label: &str, successors: &[u64]) -> fmt::Result {
    write!(f, "{label} -> ")?;
    for (index, target) in successors.iter().enumerate() {
        if index > 0 {
            write!(f, ", ")?;
        }
        write!(f, "{target:#x}")?;
    }
    Ok(())
}

impl fmt::Display for RecoveredCfg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (function_index, function) in self.functions.iter().enumerate() {
            if function_index > 0 {
                writeln!(f)?;
            }
            writeln!(f, "function {:#x}:", function.entry)?;
            for &start in &function.block_starts {
                let Some(block) = self.blocks.get(&start) else {
                    continue;
                };
                write!(
                    f,
                    "  block {:#x}..{:#x} ({} instr) ",
                    block.start, block.end, block.instruction_count
                )?;
                block.terminator.render(f, &block.successors)?;
                writeln!(f)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CodeView, EdgeKind, RecoverError, recover_cfg};
    use crate::lift_program;
    use nx86_arm64_decode::decode_program;

    const BASE: u64 = 0x1000;

    fn movz(rd: u32, imm: u16) -> [u8; 4] {
        (0xD280_0000 | (u32::from(imm) << 5) | rd).to_le_bytes()
    }

    fn svc() -> [u8; 4] {
        0xD400_0001u32.to_le_bytes()
    }

    fn b(imm26: i32) -> [u8; 4] {
        (0x1400_0000 | ((imm26 as u32) & 0x03FF_FFFF)).to_le_bytes()
    }

    fn b_eq(imm19: i32) -> [u8; 4] {
        (0x5400_0000 | (((imm19 as u32) & 0x0007_FFFF) << 5)).to_le_bytes()
    }

    /// Concatenate 4-byte instruction encodings into one program image.
    fn program(words: &[[u8; 4]]) -> Vec<u8> {
        words.iter().flatten().copied().collect()
    }

    fn recover(bytes: &[u8], entry: u64) -> super::RecoveredCfg {
        let code = CodeView::new(BASE, bytes).expect("aligned code");
        recover_cfg(&code, &[entry]).expect("recovery should succeed")
    }

    #[test]
    fn straight_line_is_one_exit_block() {
        let bytes = program(&[movz(0, 1), movz(1, 2), svc()]);
        let cfg = recover(&bytes, BASE);

        assert_eq!(cfg.blocks.len(), 1);
        let block = &cfg.blocks[&BASE];
        assert_eq!(block.instruction_count, 3);
        assert_eq!(block.end, BASE + 12);
        assert_eq!(block.terminator, EdgeKind::Exit);
        assert!(block.successors.is_empty());
        assert_eq!(cfg.functions[0].block_starts, vec![BASE]);
    }

    #[test]
    fn forward_branch_skips_dead_fallthrough() {
        // 0x1000: b +2 (-> 0x1008) ; 0x1004: movz (dead) ; 0x1008: svc
        let bytes = program(&[b(2), movz(9, 0), svc()]);
        let cfg = recover(&bytes, BASE);

        assert_eq!(cfg.blocks.len(), 2);
        assert!(
            !cfg.blocks.contains_key(&(BASE + 4)),
            "dead instr not decoded"
        );
        let head = &cfg.blocks[&BASE];
        assert_eq!(head.terminator, EdgeKind::DirectBranch);
        assert_eq!(head.successors, vec![BASE + 8]);
        assert_eq!(cfg.blocks[&(BASE + 8)].terminator, EdgeKind::Exit);
    }

    #[test]
    fn conditional_diamond_reconverges() {
        // 0x1000: b.eq +3 (-> 0x100c) ; 0x1004: movz ; 0x1008: b +2 (-> 0x1010)
        // 0x100c: movz ; 0x1010: svc
        let bytes = program(&[b_eq(3), movz(1, 1), b(2), movz(1, 2), svc()]);
        let cfg = recover(&bytes, BASE);

        assert_eq!(cfg.blocks.len(), 4);
        let head = &cfg.blocks[&BASE];
        assert_eq!(
            head.terminator,
            EdgeKind::CondBranch {
                taken: BASE + 0xc,
                not_taken: BASE + 4,
            }
        );
        assert_eq!(head.successors, vec![BASE + 0xc, BASE + 4]);
        // Both arms reconverge at the join block 0x1010.
        assert_eq!(cfg.blocks[&(BASE + 4)].successors, vec![BASE + 0x10]);
        assert_eq!(cfg.blocks[&(BASE + 0xc)].successors, vec![BASE + 0x10]);
        assert_eq!(cfg.blocks[&(BASE + 0x10)].terminator, EdgeKind::Exit);
    }

    #[test]
    fn backward_branch_forms_loop_without_diverging() {
        // 0x1000: movz ; 0x1004: b.eq +3 (-> 0x1010) ; 0x1008: movz ;
        // 0x100c: b -2 (-> 0x1004) ; 0x1010: svc
        let bytes = program(&[movz(0, 1), b_eq(3), movz(1, 2), b(-2), svc()]);
        let cfg = recover(&bytes, BASE);

        assert_eq!(cfg.blocks.len(), 4);
        // The body block branches back to the loop test at 0x1004.
        let body = &cfg.blocks[&(BASE + 8)];
        assert_eq!(body.terminator, EdgeKind::DirectBranch);
        assert_eq!(body.successors, vec![BASE + 4]);
        assert_eq!(
            cfg.functions[0].block_starts,
            vec![BASE, BASE + 4, BASE + 8, BASE + 0x10]
        );
    }

    #[test]
    fn branch_into_run_splits_the_block() {
        // 0x1000: movz ; 0x1004: movz ; 0x1008: b.eq -1 (-> 0x1004) ; 0x100c: svc
        // The branch target 0x1004 splits the entry's straight-line run.
        let bytes = program(&[movz(0, 1), movz(1, 2), b_eq(-1), svc()]);
        let cfg = recover(&bytes, BASE);

        assert_eq!(cfg.blocks.len(), 3);
        let head = &cfg.blocks[&BASE];
        assert_eq!(head.instruction_count, 1);
        assert_eq!(head.end, BASE + 4);
        assert_eq!(head.terminator, EdgeKind::Fallthrough);
        assert_eq!(head.successors, vec![BASE + 4]);
        // The split block loops back onto itself via the conditional branch.
        let split = &cfg.blocks[&(BASE + 4)];
        assert!(split.successors.contains(&(BASE + 4)));
    }

    #[test]
    fn cond_branch_to_fallthrough_has_single_edge() {
        // 0x1000: b.eq +1 (-> 0x1004, which is also the fall-through) ; 0x1004: svc
        let bytes = program(&[b_eq(1), svc()]);
        let cfg = recover(&bytes, BASE);

        let head = &cfg.blocks[&BASE];
        assert_eq!(
            head.terminator,
            EdgeKind::CondBranch {
                taken: BASE + 4,
                not_taken: BASE + 4,
            }
        );
        // taken == not_taken collapses to one successor, not a duplicated edge.
        assert_eq!(head.successors, vec![BASE + 4]);
    }

    #[test]
    fn display_renders_deterministic_cfg() {
        let bytes = program(&[b_eq(3), movz(1, 1), b(2), movz(1, 2), svc()]);
        let cfg = recover(&bytes, BASE);

        let expected = "function 0x1000:\n  \
            block 0x1000..0x1004 (1 instr) cond -> 0x100c, 0x1004\n  \
            block 0x1004..0x100c (2 instr) branch -> 0x1010\n  \
            block 0x100c..0x1010 (1 instr) fallthrough -> 0x1010\n  \
            block 0x1010..0x1014 (1 instr) exit\n";
        assert_eq!(cfg.to_string(), expected);
    }

    #[test]
    fn recovered_leaders_match_lifter_for_reachable_program() {
        // For a fully reachable, contiguous program, recovery's block starts must
        // agree with the lifter's existing linear-sweep CFG construction.
        let bytes = program(&[b_eq(3), movz(1, 1), b(2), movz(1, 2), svc()]);
        let cfg = recover(&bytes, BASE);

        let decoded = decode_program(&bytes, BASE).expect("program decodes");
        let function = lift_program("recover-consistency", &decoded, BASE).expect("program lifts");
        let mut lifter_starts: Vec<u64> = function
            .blocks
            .iter()
            .map(crate::Block::entry_address)
            .collect();
        lifter_starts.sort_unstable();

        let recovered_starts: Vec<u64> = cfg.blocks.keys().copied().collect();
        assert_eq!(recovered_starts, lifter_starts);
    }

    #[test]
    fn rejects_unaligned_and_missing_entries() {
        assert_eq!(
            CodeView::new(BASE, &[0, 1, 2]).expect_err("unaligned code rejected"),
            RecoverError::UnalignedCode { len: 3 }
        );

        let bytes = program(&[svc()]);
        let code = CodeView::new(BASE, &bytes).expect("aligned");
        assert_eq!(
            recover_cfg(&code, &[]).expect_err("missing entries rejected"),
            RecoverError::NoEntries
        );
        assert_eq!(
            recover_cfg(&code, &[BASE + 0x100]).expect_err("out-of-range entry rejected"),
            RecoverError::UndecodableEntry {
                entry: BASE + 0x100
            }
        );
    }
}
