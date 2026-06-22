//! Inspector composition for Nx86.
//!
//! The Inspector derives, from a guest program's raw bytes and an entry PC, the
//! five views Phase 27 surfaces: disassembly, the recovered function/block CFG,
//! the lifted NxIR, and the native (x86_64) mapping. It is pure analysis built
//! on top of the existing decoder, CFG recovery, lifter, and backend lowering —
//! it generates no native code execution of its own, so it builds and runs on
//! every host (including the Apple Silicon dev host).
//!
//! Decoding and CFG recovery are prerequisites: if the bytes cannot even be
//! decoded, there is nothing to inspect, and [`inspect_program`] returns an
//! error. The NxIR and native views degrade gracefully — a program that cannot
//! be lifted or lowered still yields disassembly and a recovered CFG, matching
//! the rule that the Inspector MAY inspect a title even if it cannot compile.

use std::fmt::Write as _;

use nx86_arm64_decode::{DecodeError, DecodedInstruction, decode_program};
use nx86_arm64_lift::{CodeView, RecoverError, RecoveredCfg, lift_program, recover_cfg};
use nx86_x64_v4::lower_function;
use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-inspector";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

/// A complete inspection of one guest program seeded from a single entry PC.
#[derive(Clone, Debug)]
pub struct InspectorReport {
    /// The entry PC the program was inspected from.
    pub entry: u64,
    /// Every decoded instruction, in address order.
    pub disassembly: Vec<DecodedInstruction>,
    /// Recovered functions and basic blocks reachable from the entry.
    pub cfg: RecoveredCfg,
    /// The lifted NxIR, or the reason it is unavailable.
    pub nxir: NxirView,
    /// The native (x86_64) mapping, or the reason it is unavailable.
    pub native: NativeView,
}

/// The NxIR view: either a rendered dump or the reason lifting failed.
#[derive(Clone, Debug)]
pub enum NxirView {
    Dump(String),
    Unavailable(String),
}

/// The native mapping view: per-block lowered code, or the reason it is absent.
#[derive(Clone, Debug)]
pub enum NativeView {
    Mapped(Vec<NativeBlockMapping>),
    Unavailable(String),
}

/// One guest block lowered to native code, keyed by its guest entry PC.
#[derive(Clone, Debug)]
pub struct NativeBlockMapping {
    /// Guest entry PC of the block (its dispatcher key).
    pub entry_pc: u64,
    /// Number of native bytes emitted for the block.
    pub byte_len: usize,
    /// Human-readable x86_64 disassembly of the block.
    pub dump: String,
}

#[derive(Debug, Error)]
pub enum InspectError {
    #[error("decode failed: {0}")]
    Decode(#[from] DecodeError),
    #[error("cfg recovery failed: {0}")]
    Recover(#[from] RecoverError),
}

/// Inspect a guest program located at `entry`.
///
/// Decoding and CFG recovery must succeed; the NxIR and native views are
/// best-effort and report a reason when unavailable rather than failing.
pub fn inspect_program(bytes: &[u8], entry: u64) -> Result<InspectorReport, InspectError> {
    let disassembly = decode_program(bytes, entry)?;
    let cfg = recover_cfg(&CodeView::new(entry, bytes)?, &[entry])?;

    // Lift once; both the NxIR dump and the native mapping derive from it.
    let lift = lift_program("inspected", &disassembly, entry);
    let nxir = match &lift {
        Ok(function) => NxirView::Dump(function.dump()),
        Err(error) => NxirView::Unavailable(error.to_string()),
    };
    let native = match &lift {
        Ok(function) => match lower_function(function) {
            Ok(blocks) => NativeView::Mapped(
                blocks
                    .iter()
                    .map(|block| NativeBlockMapping {
                        entry_pc: block.entry_pc,
                        byte_len: block.lowered.bytes().len(),
                        dump: block.lowered.dump().to_owned(),
                    })
                    .collect(),
            ),
            Err(error) => NativeView::Unavailable(error.to_string()),
        },
        Err(error) => NativeView::Unavailable(format!("native mapping needs NxIR: {error}")),
    };

    Ok(InspectorReport {
        entry,
        disassembly,
        cfg,
        nxir,
        native,
    })
}

impl InspectorReport {
    /// Render the disassembly as `address  mnemonic` lines.
    #[must_use]
    pub fn disassembly_text(&self) -> String {
        let mut output = String::new();
        for inst in &self.disassembly {
            let _ = writeln!(output, "{:#010x}  {}", inst.address, inst.disassembly);
        }
        output
    }

    /// Render the recovered functions as one summary line each.
    #[must_use]
    pub fn function_list_text(&self) -> String {
        let mut output = String::new();
        for function in &self.cfg.functions {
            let _ = writeln!(
                output,
                "function @{:#x} — {} block(s)",
                function.entry,
                function.block_starts.len()
            );
        }
        output
    }

    /// Render the NxIR view (the dump, or its unavailable reason).
    #[must_use]
    pub fn nxir_text(&self) -> String {
        match &self.nxir {
            NxirView::Dump(dump) => dump.clone(),
            NxirView::Unavailable(reason) => format!("NxIR unavailable: {reason}"),
        }
    }

    /// Render the native mapping view (per-block dumps, or its reason).
    #[must_use]
    pub fn native_text(&self) -> String {
        match &self.native {
            NativeView::Mapped(blocks) => {
                let mut output = String::new();
                for block in blocks {
                    let _ = writeln!(
                        output,
                        "block @{:#x} ({} bytes):",
                        block.entry_pc, block.byte_len
                    );
                    for line in block.dump.lines() {
                        let _ = writeln!(output, "  {line}");
                    }
                }
                output
            }
            NativeView::Unavailable(reason) => format!("native mapping unavailable: {reason}"),
        }
    }

    /// A single deterministic text report combining every view, suitable for
    /// persisting under a title's `inspector/` directory.
    #[must_use]
    pub fn render_text(&self) -> String {
        let mut output = String::new();
        let _ = writeln!(output, "entry {:#x}", self.entry);
        let _ = writeln!(output, "\n== disassembly ==\n{}", self.disassembly_text());
        let _ = writeln!(output, "== cfg ==\n{}", self.cfg);
        let _ = writeln!(output, "== nxir ==\n{}", self.nxir_text());
        let _ = writeln!(output, "== native ==\n{}", self.native_text());
        output
    }
}

#[cfg(test)]
mod tests {
    use super::{InspectError, NativeView, NxirView, inspect_program};

    // MOVZ x0, #1   -> 0xD2800020
    // SVC  #0       -> 0xD4000001
    fn straight_line() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0xD280_0020_u32.to_le_bytes());
        bytes.extend_from_slice(&0xD400_0001_u32.to_le_bytes());
        bytes
    }

    #[test]
    fn inspects_straight_line_program() {
        let report = inspect_program(&straight_line(), 0).expect("inspection should succeed");

        assert_eq!(report.entry, 0);
        assert_eq!(report.disassembly.len(), 2);
        // One function, one block (entry to SVC exit), reachable from the entry.
        assert_eq!(report.cfg.functions.len(), 1);
        assert_eq!(report.cfg.functions[0].entry, 0);
        assert_eq!(report.cfg.blocks.len(), 1);

        // NxIR lifts and the dump is non-empty.
        match &report.nxir {
            NxirView::Dump(dump) => assert!(dump.contains("fn inspected")),
            NxirView::Unavailable(reason) => panic!("expected NxIR dump, got: {reason}"),
        }

        // Native lowering is host-independent (pure byte emission): either it
        // maps the block to non-empty native bytes, or it reports a clean
        // unavailable reason — never a panic.
        match &report.native {
            NativeView::Mapped(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert!(blocks[0].byte_len > 0);
                assert!(!blocks[0].dump.is_empty());
            }
            NativeView::Unavailable(reason) => assert!(!reason.is_empty()),
        }
    }

    #[test]
    fn forward_branch_prunes_dead_fallthrough() {
        // B #8 (skip the next instr), MOVZ x0,#1 (dead), SVC #0.
        // B at 0x0 with imm26 = 2 words -> 0x14000002.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x1400_0002_u32.to_le_bytes());
        bytes.extend_from_slice(&0xD280_0020_u32.to_le_bytes());
        bytes.extend_from_slice(&0xD400_0001_u32.to_le_bytes());

        let report = inspect_program(&bytes, 0).expect("inspection should succeed");

        // The dead fall-through instruction is never reached, so it is not a
        // recovered block leader: two blocks (the branch and the target), and
        // the dead MOVZ is pruned.
        assert_eq!(report.cfg.functions.len(), 1);
        assert_eq!(report.cfg.blocks.len(), 2);
        assert!(report.cfg.blocks.contains_key(&0));
        assert!(report.cfg.blocks.contains_key(&8));
        assert!(!report.cfg.blocks.contains_key(&4));
    }

    #[test]
    fn render_text_contains_every_section() {
        let report = inspect_program(&straight_line(), 0).expect("inspection should succeed");
        let text = report.render_text();

        for section in [
            "== disassembly ==",
            "== cfg ==",
            "== nxir ==",
            "== native ==",
        ] {
            assert!(text.contains(section), "missing section: {section}");
        }
    }

    #[test]
    fn misaligned_length_is_reported() {
        let error = inspect_program(&[0x00, 0x01, 0x02], 0).expect_err("odd length should fail");
        assert!(matches!(error, InspectError::Decode(_)));
    }
}
