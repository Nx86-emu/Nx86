//! Emergency single-block JIT compilation (Phase 23).

use nx86_cache::{CacheError, CacheManager};
use nx86_ir::{Function, verify};
use nx86_object::NativeObject;
use nx86_x64_v4::{LoweringError, block_entry_pc, lower_function_block};
use thiserror::Error;

/// One emergency-JIT event suitable for runtime diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitEvent {
    pub guest_pc: u64,
    pub code_size_bytes: usize,
    pub cache_file_name: String,
}

/// A newly compiled block and the event describing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitCompilation {
    pub object: NativeObject,
    pub event: JitEvent,
    /// Halt reason for this block, when its terminator halts execution.
    pub halt_reason: Option<String>,
}

/// On-demand compiler for missing native blocks from one verified NxIR function.
#[derive(Clone, Debug)]
pub struct EmergencyJit {
    function: Function,
    cache: CacheManager,
}

impl EmergencyJit {
    /// Create an emergency JIT over a verified source function and object cache.
    pub fn new(function: Function, cache: CacheManager) -> Result<Self, JitError> {
        verify::verify(&function)?;
        Ok(Self { function, cache })
    }

    /// Compile and cache the block at `guest_pc`.
    ///
    /// Returns `Ok(None)` when the source function has no block at that PC.
    pub fn compile(&self, guest_pc: u64) -> Result<Option<JitCompilation>, JitError> {
        let Some(source_block) = self
            .function
            .blocks
            .iter()
            .find(|block| block_entry_pc(block) == guest_pc)
        else {
            return Ok(None);
        };
        let Some(compiled) = lower_function_block(&self.function, guest_pc)? else {
            return Ok(None);
        };
        let stack_size = u32::try_from(compiled.lowered.stack_size()).map_err(|_| {
            JitError::InvalidStackSize {
                stack_size: compiled.lowered.stack_size(),
            }
        })?;
        let object = NativeObject {
            entry_address: guest_pc,
            guest_end: source_block.terminator_address.saturating_add(4),
            stack_size,
            code: compiled.lowered.bytes().to_vec(),
        };
        let cache_entry = self.cache.insert(&object)?;
        let event = JitEvent {
            guest_pc,
            code_size_bytes: object.code.len(),
            cache_file_name: cache_entry.file_name,
        };
        tracing::info!(
            guest_pc,
            code_size_bytes = event.code_size_bytes,
            cache_file = %event.cache_file_name,
            "emergency JIT compiled missing block"
        );
        let halt_reason = match &source_block.terminator {
            nx86_ir::Terminator::Halt { reason } => Some(reason.clone()),
            nx86_ir::Terminator::Branch { .. }
            | nx86_ir::Terminator::CondBranch { .. }
            | nx86_ir::Terminator::Return => None,
        };
        Ok(Some(JitCompilation {
            object,
            event,
            halt_reason,
        }))
    }
}

/// A failure preparing or compiling an emergency-JIT block.
#[derive(Debug, Error)]
pub enum JitError {
    #[error("emergency JIT source function failed verification: {0}")]
    InvalidIr(#[from] verify::VerifyError),
    #[error("emergency JIT lowering failed: {0}")]
    Lowering(#[from] LoweringError),
    #[error("emergency JIT cache operation failed: {0}")]
    Cache(#[from] CacheError),
    #[error("emergency JIT lowerer returned invalid stack size {stack_size}")]
    InvalidStackSize { stack_size: i32 },
}

#[cfg(test)]
mod tests {
    use nx86_cache::CacheManager;
    use nx86_ir::{Block, BlockId, Function, Inst, Op, Reg, Terminator, Type, Value};
    use tempfile::tempdir;

    use super::EmergencyJit;

    #[test]
    fn compiles_missing_block_into_cache_and_logs_event() {
        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        let jit = EmergencyJit::new(two_block_function(), cache.clone()).expect("create JIT");

        let compilation = jit
            .compile(0x8)
            .expect("compile block")
            .expect("block should exist");

        assert_eq!(compilation.object.entry_address, 0x8);
        assert_eq!(compilation.object.guest_end, 0xc);
        assert_eq!(compilation.event.guest_pc, 0x8);
        assert_eq!(
            compilation.event.code_size_bytes,
            compilation.object.code.len()
        );
        assert_eq!(compilation.event.cache_file_name, "0000000000000008.nxo");
        assert_eq!(compilation.halt_reason.as_deref(), Some("svc #0x0"));
        assert_eq!(
            cache.load(0x8).expect("load cached block"),
            compilation.object
        );
    }

    #[test]
    fn unknown_guest_pc_does_not_touch_cache() {
        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        let jit = EmergencyJit::new(two_block_function(), cache.clone()).expect("create JIT");

        assert!(jit.compile(0xdead).expect("look up block").is_none());
        assert_eq!(cache.scan().expect("scan cache").object_count(), 0);
    }

    fn two_block_function() -> Function {
        Function {
            name: "jit_two_block".to_owned(),
            entry_address: 0,
            value_count: 2,
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
