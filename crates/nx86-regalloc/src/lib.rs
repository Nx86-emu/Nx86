//! Basic register allocator (Phase 19).
//!
//! A linear-scan allocator over a single NxIR block. Each SSA value lives from
//! its definition to its last use and is given the lowest-index free pool
//! register at definition; when the pool is exhausted the value spills to a
//! fresh stack slot. The scan is deterministic so generated code and dumps stay
//! stable. The allocator is pure logic (host-independent): it only decides
//! *where* each value lives; emitting the moves is the lowerer's job.

use std::collections::HashMap;

use nx86_ir::{Block, Value};
use nx86_x64_asm::Reg64;

pub const CRATE_NAME: &str = "nx86-regalloc";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

const POOL_SIZE: usize = 6;

/// The caller-saved x86-64 registers the allocator may assign to SSA values.
///
/// RDI holds the `NativeBlockState` pointer for the whole block, RSP/RBP frame
/// the stack, and RAX/RCX are reserved as fixed scratch by the lowerer, so none
/// of those appear here. Every register in the pool is caller-saved, so the
/// generated leaf block never has to save or restore them.
const POOL: [Reg64; POOL_SIZE] = [
    Reg64::Rdx,
    Reg64::Rsi,
    Reg64::R8,
    Reg64::R9,
    Reg64::R10,
    Reg64::R11,
];

/// Where a single SSA value lives for the duration of a native block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    /// Assigned to a physical x86-64 register from the allocatable pool.
    Register(Reg64),
    /// Spilled to stack slot `index` (0-based); the lowerer maps this to a
    /// frame offset.
    Spill(u32),
}

/// The result of allocating one block: a location for every defined SSA value
/// plus the number of stack slots the lowerer must reserve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allocation {
    locations: Vec<Option<Location>>,
    spill_count: u32,
}

impl Allocation {
    /// The location chosen for `value`, or `None` if it is never defined in the
    /// block (e.g. an out-of-range value index).
    #[must_use]
    pub fn location(&self, value: Value) -> Option<Location> {
        self.locations.get(value.0 as usize).copied().flatten()
    }

    /// Number of stack spill slots the block needs.
    #[must_use]
    pub const fn spill_count(&self) -> u32 {
        self.spill_count
    }
}

/// Allocate registers for `block`, whose function declares `value_count` SSA
/// values. See the module docs for the algorithm.
#[must_use]
pub fn allocate(block: &Block, value_count: u32) -> Allocation {
    // Last instruction index that uses each value. Results that are never used
    // get their definition index below, so every live value can expire.
    let mut last_use: HashMap<Value, usize> = HashMap::new();
    for (index, inst) in block.instructions.iter().enumerate() {
        for operand in inst.op.operands() {
            last_use.insert(operand, index);
        }
    }

    let mut locations: Vec<Option<Location>> = vec![None; value_count as usize];
    let mut occupied: [Option<Value>; POOL_SIZE] = [None; POOL_SIZE];
    let mut spill_count: u32 = 0;

    for (index, inst) in block.instructions.iter().enumerate() {
        // Free pool registers whose value was last used before this instruction.
        for slot in &mut occupied {
            if let Some(value) = *slot
                && last_use.get(&value).copied().unwrap_or(index) < index
            {
                *slot = None;
            }
        }

        let Some(result) = inst.result else {
            continue;
        };
        // A result that is never read still dies at its own definition.
        last_use.entry(result).or_insert(index);

        let location = match occupied.iter().position(Option::is_none) {
            Some(free) => {
                occupied[free] = Some(result);
                Location::Register(POOL[free])
            }
            None => {
                let slot = spill_count;
                spill_count += 1;
                Location::Spill(slot)
            }
        };
        if let Some(entry) = locations.get_mut(result.0 as usize) {
            *entry = Some(location);
        }
    }

    Allocation {
        locations,
        spill_count,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use nx86_ir::{Block, Inst, Op, Reg, Terminator, Type, Value};

    use super::{Location, allocate};

    fn const_inst(result: u32, value: u64) -> Inst {
        Inst {
            result: Some(Value(result)),
            op: Op::Const {
                ty: Type::I64,
                value,
            },
            guest_address: 0,
        }
    }

    fn setreg_inst(reg: u8, value: u32) -> Inst {
        Inst {
            result: None,
            op: Op::SetReg {
                reg: Reg::X(reg),
                value: Value(value),
            },
            guest_address: 0,
        }
    }

    fn block(instructions: Vec<Inst>) -> Block {
        Block {
            instructions,
            terminator: Terminator::Halt {
                reason: "svc #0x0".to_owned(),
            },
            terminator_address: 0,
        }
    }

    #[test]
    fn assigns_registers_when_pool_suffices() {
        // Six values defined first, then each consumed: all six are live at
        // once, exactly filling the pool with no spill.
        let mut instructions = Vec::new();
        for index in 0u32..6 {
            instructions.push(const_inst(index, u64::from(index)));
        }
        for index in 0u32..6 {
            instructions.push(setreg_inst(index as u8, index));
        }

        let allocation = allocate(&block(instructions), 6);

        assert_eq!(allocation.spill_count(), 0);
        let registers: HashSet<&str> = (0u32..6)
            .filter_map(|index| match allocation.location(Value(index)) {
                Some(Location::Register(reg)) => Some(reg.name()),
                _ => None,
            })
            .collect();
        assert_eq!(registers.len(), 6, "each value gets a distinct register");
    }

    #[test]
    fn spills_when_pool_exhausted() {
        // Seven values live at once: six registers plus one spill.
        let mut instructions = Vec::new();
        for index in 0u32..7 {
            instructions.push(const_inst(index, u64::from(index)));
        }
        for index in 0u32..7 {
            instructions.push(setreg_inst(index as u8, index));
        }

        let allocation = allocate(&block(instructions), 7);

        assert_eq!(allocation.spill_count(), 1);
        let registers = (0u32..7)
            .filter(|&index| {
                matches!(
                    allocation.location(Value(index)),
                    Some(Location::Register(_))
                )
            })
            .count();
        assert_eq!(registers, 6);
        assert_eq!(allocation.location(Value(6)), Some(Location::Spill(0)));
    }

    #[test]
    fn reuses_registers_after_last_use() {
        // Define and immediately consume each value, so only one is ever live:
        // twelve values, zero spills, all in registers.
        let mut instructions = Vec::new();
        for index in 0u32..12 {
            instructions.push(const_inst(index, u64::from(index)));
            instructions.push(setreg_inst(0, index));
        }

        let allocation = allocate(&block(instructions), 12);

        assert_eq!(allocation.spill_count(), 0);
        for index in 0u32..12 {
            assert!(matches!(
                allocation.location(Value(index)),
                Some(Location::Register(_))
            ));
        }
    }
}
