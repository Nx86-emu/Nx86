use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-x64-asm";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBuffer {
    bytes: Vec<u8>,
    dump: String,
    patch_sites: Vec<PatchSite>,
}

impl CodeBuffer {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    #[must_use]
    pub fn dump(&self) -> &str {
        &self.dump
    }

    /// Runtime-patchable sites recorded during assembly (e.g. block-chain exits).
    #[must_use]
    pub fn patch_sites(&self) -> &[PatchSite] {
        &self.patch_sites
    }
}

/// What a [`PatchSite`] can be overwritten with at runtime (SPEC §23.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchKind {
    /// A block's unconditional exit slot, sized to hold a `jmp rel32` so a hot
    /// edge can be chained directly to its successor block.
    ChainExit,
}

/// A location in emitted code that may be rewritten after assembly, recorded with
/// its byte `offset` (from the start of the code buffer) and fixed `size`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatchSite {
    pub offset: usize,
    pub size: usize,
    pub kind: PatchKind,
}

/// Size of a chain-exit slot: a 5-byte region (`jmp rel32`) that holds a `ret`
/// plus padding until it is patched to a direct jump.
pub const CHAIN_EXIT_SIZE: usize = 5;

/// Encode a near `jmp rel32` whose displacement is `rel` (target minus the
/// address of the byte after this 5-byte instruction).
#[must_use]
pub const fn encode_jmp_rel32(rel: i32) -> [u8; CHAIN_EXIT_SIZE] {
    let disp = rel.to_le_bytes();
    [0xE9, disp[0], disp[1], disp[2], disp[3]]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label(usize);

impl Label {
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reg64 {
    Rax,
    Rcx,
    Rdx,
    Rbx,
    Rsp,
    Rbp,
    Rsi,
    Rdi,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
}

impl Reg64 {
    const fn number(self) -> u8 {
        match self {
            Self::Rax => 0,
            Self::Rcx => 1,
            Self::Rdx => 2,
            Self::Rbx => 3,
            Self::Rsp => 4,
            Self::Rbp => 5,
            Self::Rsi => 6,
            Self::Rdi => 7,
            Self::R8 => 8,
            Self::R9 => 9,
            Self::R10 => 10,
            Self::R11 => 11,
            Self::R12 => 12,
            Self::R13 => 13,
            Self::R14 => 14,
            Self::R15 => 15,
        }
    }

    const fn low3(self) -> u8 {
        self.number() & 0b111
    }

    const fn rex_bit(self) -> bool {
        self.number() & 0b1000 != 0
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rax => "rax",
            Self::Rcx => "rcx",
            Self::Rdx => "rdx",
            Self::Rbx => "rbx",
            Self::Rsp => "rsp",
            Self::Rbp => "rbp",
            Self::Rsi => "rsi",
            Self::Rdi => "rdi",
            Self::R8 => "r8",
            Self::R9 => "r9",
            Self::R10 => "r10",
            Self::R11 => "r11",
            Self::R12 => "r12",
            Self::R13 => "r13",
            Self::R14 => "r14",
            Self::R15 => "r15",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegXmm {
    Xmm0,
    Xmm1,
    Xmm2,
    Xmm3,
    Xmm4,
    Xmm5,
    Xmm6,
    Xmm7,
    Xmm8,
    Xmm9,
    Xmm10,
    Xmm11,
    Xmm12,
    Xmm13,
    Xmm14,
    Xmm15,
}

impl RegXmm {
    const fn number(self) -> u8 {
        match self {
            Self::Xmm0 => 0,
            Self::Xmm1 => 1,
            Self::Xmm2 => 2,
            Self::Xmm3 => 3,
            Self::Xmm4 => 4,
            Self::Xmm5 => 5,
            Self::Xmm6 => 6,
            Self::Xmm7 => 7,
            Self::Xmm8 => 8,
            Self::Xmm9 => 9,
            Self::Xmm10 => 10,
            Self::Xmm11 => 11,
            Self::Xmm12 => 12,
            Self::Xmm13 => 13,
            Self::Xmm14 => 14,
            Self::Xmm15 => 15,
        }
    }

    const fn low3(self) -> u8 {
        self.number() & 0b111
    }

    const fn rex_bit(self) -> bool {
        self.number() & 0b1000 != 0
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Xmm0 => "xmm0",
            Self::Xmm1 => "xmm1",
            Self::Xmm2 => "xmm2",
            Self::Xmm3 => "xmm3",
            Self::Xmm4 => "xmm4",
            Self::Xmm5 => "xmm5",
            Self::Xmm6 => "xmm6",
            Self::Xmm7 => "xmm7",
            Self::Xmm8 => "xmm8",
            Self::Xmm9 => "xmm9",
            Self::Xmm10 => "xmm10",
            Self::Xmm11 => "xmm11",
            Self::Xmm12 => "xmm12",
            Self::Xmm13 => "xmm13",
            Self::Xmm14 => "xmm14",
            Self::Xmm15 => "xmm15",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegMask {
    K0,
    K1,
    K2,
    K3,
    K4,
    K5,
    K6,
    K7,
}

impl RegMask {
    const fn number(self) -> u8 {
        match self {
            Self::K0 => 0,
            Self::K1 => 1,
            Self::K2 => 2,
            Self::K3 => 3,
            Self::K4 => 4,
            Self::K5 => 5,
            Self::K6 => 6,
            Self::K7 => 7,
        }
    }

    const fn low3(self) -> u8 {
        self.number() & 0b111
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::K0 => "k0",
            Self::K1 => "k1",
            Self::K2 => "k2",
            Self::K3 => "k3",
            Self::K4 => "k4",
            Self::K5 => "k5",
            Self::K6 => "k6",
            Self::K7 => "k7",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mem64 {
    pub base: Reg64,
    pub index: Option<Reg64>,
    pub disp: i32,
}

impl Mem64 {
    #[must_use]
    pub const fn new(base: Reg64, disp: i32) -> Self {
        Self {
            base,
            index: None,
            disp,
        }
    }

    #[must_use]
    pub const fn indexed(base: Reg64, index: Reg64, disp: i32) -> Self {
        Self {
            base,
            index: Some(index),
            disp,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AsmError {
    #[error("label {label} does not exist")]
    InvalidLabel { label: usize },
    #[error("label {label} was already bound")]
    DuplicateLabel { label: usize },
    #[error("label {label} was not bound")]
    UnresolvedLabel { label: usize },
    #[error("relative jump from {source_ip} to {target} does not fit in i32")]
    RelativeOutOfRange { source_ip: usize, target: usize },
}

#[derive(Default)]
pub struct Assembler {
    bytes: Vec<u8>,
    dump: Vec<String>,
    labels: Vec<LabelState>,
    patch_sites: Vec<PatchSite>,
}

#[derive(Default)]
struct LabelState {
    position: Option<usize>,
    fixups: Vec<Rel32Fixup>,
}

#[derive(Clone, Copy)]
struct Rel32Fixup {
    offset: usize,
    next_ip: usize,
}

impl Assembler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn create_label(&mut self) -> Label {
        let label = Label(self.labels.len());
        self.labels.push(LabelState::default());
        label
    }

    pub fn bind_label(&mut self, label: Label) -> Result<(), AsmError> {
        let index = label.0;
        let Some(state) = self.labels.get_mut(index) else {
            return Err(AsmError::InvalidLabel { label: label.0 });
        };
        if state.position.is_some() {
            return Err(AsmError::DuplicateLabel { label: label.0 });
        }
        state.position = Some(self.bytes.len());
        self.dump.push(format!(".L{}:", label.0));
        Ok(())
    }

    pub fn prologue(&mut self) {
        self.dump.push("push rbp".to_owned());
        self.push_reg_raw(Reg64::Rbp);
        self.mov_reg_reg(Reg64::Rbp, Reg64::Rsp);
    }

    pub fn epilogue(&mut self) {
        self.dump.push("pop rbp".to_owned());
        self.pop_reg_raw(Reg64::Rbp);
        self.ret();
    }

    /// A `pop rbp` followed by a chain-exit slot instead of a bare `ret`. The
    /// frame is fully torn down before the slot, so when the slot is later
    /// patched to a direct `jmp` the successor block sets up its own frame and
    /// the chain stays stack-balanced.
    pub fn chain_epilogue(&mut self) {
        self.dump.push("pop rbp".to_owned());
        self.pop_reg_raw(Reg64::Rbp);
        self.chain_exit();
    }

    /// Emit a [`PatchKind::ChainExit`] slot: a `ret` padded with `nop`s to
    /// [`CHAIN_EXIT_SIZE`] bytes, and record the patch site. Unpatched, the `ret`
    /// returns to the dispatcher; patched, the whole slot becomes a `jmp rel32`.
    pub fn chain_exit(&mut self) {
        let offset = self.bytes.len();
        self.patch_sites.push(PatchSite {
            offset,
            size: CHAIN_EXIT_SIZE,
            kind: PatchKind::ChainExit,
        });
        self.dump.push("ret ; chain-exit slot".to_owned());
        self.bytes.push(0xC3);
        for _ in 1..CHAIN_EXIT_SIZE {
            self.nop();
        }
    }

    pub fn nop(&mut self) {
        self.dump.push("nop".to_owned());
        self.bytes.push(0x90);
    }

    pub fn ret(&mut self) {
        self.dump.push("ret".to_owned());
        self.bytes.push(0xC3);
    }

    pub fn mov_reg_imm64(&mut self, dst: Reg64, value: u64) {
        self.dump.push(format!("mov {}, {value:#x}", dst.name()));
        emit_rex(&mut self.bytes, true, None, None, Some(dst));
        self.bytes.push(0xB8 + dst.low3());
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn mov_reg_reg(&mut self, dst: Reg64, src: Reg64) {
        self.dump
            .push(format!("mov {}, {}", dst.name(), src.name()));
        self.emit_reg_reg(0x89, dst, src);
    }

    /// Move the low 32 bits and zero the destination's upper 32 bits.
    pub fn mov_reg_reg32(&mut self, dst: Reg64, src: Reg64) {
        self.dump
            .push(format!("mov {}d, {}d", dst.name(), src.name()));
        self.emit_reg_reg_width(0x89, dst, src, false);
    }

    pub fn mov_reg_mem(&mut self, dst: Reg64, src: Mem64) {
        self.dump
            .push(format!("mov {}, {}", dst.name(), mem_name(src)));
        self.emit_reg_mem(0x8B, dst, src);
    }

    /// Load a 32-bit value and zero the upper half of the destination register.
    pub fn mov_reg_mem32(&mut self, dst: Reg64, src: Mem64) {
        self.dump
            .push(format!("mov {}d, dword {}", dst.name(), mem_name(src)));
        self.emit_reg_mem_width(0x8B, dst, src, false);
    }

    pub fn movzx_reg_mem8(&mut self, dst: Reg64, src: Mem64) {
        self.dump
            .push(format!("movzx {}, byte {}", dst.name(), mem_name(src)));
        emit_rex(&mut self.bytes, true, Some(dst), src.index, Some(src.base));
        self.bytes.extend_from_slice(&[0x0F, 0xB6]);
        emit_mem_modrm(&mut self.bytes, dst.low3(), src);
    }

    pub fn mov_mem_reg(&mut self, dst: Mem64, src: Reg64) {
        self.dump
            .push(format!("mov {}, {}", mem_name(dst), src.name()));
        self.emit_mem_reg(0x89, dst, src);
    }

    pub fn mov_mem_reg32(&mut self, dst: Mem64, src: Reg64) {
        self.dump
            .push(format!("mov dword {}, {}d", mem_name(dst), src.name()));
        self.emit_mem_reg_width(0x89, dst, src, false);
    }

    pub fn movsd_xmm_mem(&mut self, dst: RegXmm, src: Mem64) {
        self.dump
            .push(format!("movsd {}, qword {}", dst.name(), mem_name(src)));
        self.emit_xmm_mem(&[0xF2], 0x10, dst, src);
    }

    pub fn movsd_mem_xmm(&mut self, dst: Mem64, src: RegXmm) {
        self.dump
            .push(format!("movsd qword {}, {}", mem_name(dst), src.name()));
        self.emit_mem_xmm(&[0xF2], 0x11, dst, src);
    }

    pub fn movdqu_xmm_mem(&mut self, dst: RegXmm, src: Mem64) {
        self.dump
            .push(format!("movdqu {}, xmmword {}", dst.name(), mem_name(src)));
        self.emit_xmm_mem(&[0xF3], 0x6F, dst, src);
    }

    pub fn movdqu_mem_xmm(&mut self, dst: Mem64, src: RegXmm) {
        self.dump
            .push(format!("movdqu xmmword {}, {}", mem_name(dst), src.name()));
        self.emit_mem_xmm(&[0xF3], 0x7F, dst, src);
    }

    pub fn addsd_xmm_mem(&mut self, dst: RegXmm, src: Mem64) {
        self.dump
            .push(format!("addsd {}, qword {}", dst.name(), mem_name(src)));
        self.emit_xmm_mem(&[0xF2], 0x58, dst, src);
    }

    pub fn subsd_xmm_mem(&mut self, dst: RegXmm, src: Mem64) {
        self.dump
            .push(format!("subsd {}, qword {}", dst.name(), mem_name(src)));
        self.emit_xmm_mem(&[0xF2], 0x5C, dst, src);
    }

    pub fn mulsd_xmm_mem(&mut self, dst: RegXmm, src: Mem64) {
        self.dump
            .push(format!("mulsd {}, qword {}", dst.name(), mem_name(src)));
        self.emit_xmm_mem(&[0xF2], 0x59, dst, src);
    }

    pub fn divsd_xmm_mem(&mut self, dst: RegXmm, src: Mem64) {
        self.dump
            .push(format!("divsd {}, qword {}", dst.name(), mem_name(src)));
        self.emit_xmm_mem(&[0xF2], 0x5E, dst, src);
    }

    pub fn addpd_xmm_mem(&mut self, dst: RegXmm, src: Mem64) {
        self.dump
            .push(format!("addpd {}, xmmword {}", dst.name(), mem_name(src)));
        self.emit_xmm_mem(&[0x66], 0x58, dst, src);
    }

    pub fn paddq_xmm_mem(&mut self, dst: RegXmm, src: Mem64) {
        self.dump
            .push(format!("paddq {}, xmmword {}", dst.name(), mem_name(src)));
        self.emit_xmm_mem(&[0x66], 0xD4, dst, src);
    }

    pub fn pcmpeqq_xmm_mem(&mut self, dst: RegXmm, src: Mem64) {
        self.dump
            .push(format!("pcmpeqq {}, xmmword {}", dst.name(), mem_name(src)));
        self.emit_xmm_mem_0f38(&[0x66], 0x29, dst, src);
    }

    pub fn pshufd_xmm_mem_imm8(&mut self, dst: RegXmm, src: Mem64, imm: u8) {
        self.dump.push(format!(
            "pshufd {}, xmmword {}, {imm:#x}",
            dst.name(),
            mem_name(src)
        ));
        self.emit_xmm_mem_imm8(&[0x66], 0x70, dst, src, imm);
    }

    pub fn vmovdqu64_xmm_mem(&mut self, dst: RegXmm, src: Mem64) {
        self.dump.push(format!(
            "vmovdqu64 {}, xmmword {}",
            dst.name(),
            mem_name(src)
        ));
        self.emit_evex_xmm_mem(EvexSpec::map_0f(0x02, None), 0x6F, dst, src);
    }

    pub fn vmovdqu64_mem_xmm(&mut self, dst: Mem64, src: RegXmm) {
        self.dump.push(format!(
            "vmovdqu64 xmmword {}, {}",
            mem_name(dst),
            src.name()
        ));
        self.emit_evex_mem_xmm(EvexSpec::map_0f(0x02, None), 0x7F, dst, src);
    }

    pub fn vpcmpeqq_mask_xmm_xmm(&mut self, dst: RegMask, lhs: RegXmm, rhs: RegXmm) {
        self.dump.push(format!(
            "vpcmpeqq {}, {}, {}",
            dst.name(),
            lhs.name(),
            rhs.name()
        ));
        self.emit_evex_prefix(
            EvexSpec::map_0f38(0x01, Some(lhs)),
            dst.number(),
            None,
            rhs.number(),
        );
        self.bytes.push(0x29);
        self.bytes.push(modrm(0b11, dst.low3(), rhs.low3()));
    }

    pub fn vpmovm2q_xmm_mask(&mut self, dst: RegXmm, src: RegMask) {
        self.dump
            .push(format!("vpmovm2q {}, {}", dst.name(), src.name()));
        self.emit_evex_prefix(
            EvexSpec::map_0f38(0x02, None),
            dst.number(),
            None,
            src.number(),
        );
        self.bytes.push(0x38);
        self.bytes.push(modrm(0b11, dst.low3(), src.low3()));
    }

    pub fn add_reg_reg(&mut self, dst: Reg64, src: Reg64) {
        self.dump
            .push(format!("add {}, {}", dst.name(), src.name()));
        self.emit_reg_reg(0x01, dst, src);
    }

    pub fn add_reg_imm32(&mut self, dst: Reg64, value: i32) {
        self.dump.push(format!("add {}, {value:#x}", dst.name()));
        self.emit_reg_imm32(0, dst, value);
    }

    pub fn sub_reg_reg(&mut self, dst: Reg64, src: Reg64) {
        self.dump
            .push(format!("sub {}, {}", dst.name(), src.name()));
        self.emit_reg_reg(0x29, dst, src);
    }

    pub fn sub_reg_imm32(&mut self, dst: Reg64, value: i32) {
        self.dump.push(format!("sub {}, {value:#x}", dst.name()));
        self.emit_reg_imm32(5, dst, value);
    }

    pub fn cmp_reg_reg(&mut self, lhs: Reg64, rhs: Reg64) {
        self.dump
            .push(format!("cmp {}, {}", lhs.name(), rhs.name()));
        self.emit_reg_reg(0x39, lhs, rhs);
    }

    pub fn cmp_reg_imm32(&mut self, lhs: Reg64, value: i32) {
        self.dump.push(format!("cmp {}, {value:#x}", lhs.name()));
        self.emit_reg_imm32(7, lhs, value);
    }

    pub fn test_reg_imm32(&mut self, reg: Reg64, value: i32) {
        self.dump.push(format!("test {}, {value:#x}", reg.name()));
        emit_rex(&mut self.bytes, true, None, None, Some(reg));
        self.bytes.push(0xF7);
        self.bytes.push(modrm(0b11, 0, reg.low3()));
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn and_reg_reg(&mut self, dst: Reg64, src: Reg64) {
        self.dump
            .push(format!("and {}, {}", dst.name(), src.name()));
        self.emit_reg_reg(0x21, dst, src);
    }

    pub fn or_reg_reg(&mut self, dst: Reg64, src: Reg64) {
        self.dump.push(format!("or {}, {}", dst.name(), src.name()));
        self.emit_reg_reg(0x09, dst, src);
    }

    pub fn xor_reg_reg(&mut self, dst: Reg64, src: Reg64) {
        self.dump
            .push(format!("xor {}, {}", dst.name(), src.name()));
        self.emit_reg_reg(0x31, dst, src);
    }

    pub fn and_reg_imm32(&mut self, dst: Reg64, value: i32) {
        self.dump.push(format!("and {}, {value:#x}", dst.name()));
        self.emit_reg_imm32(4, dst, value);
    }

    pub fn shr_reg_imm8(&mut self, dst: Reg64, value: u8) {
        self.dump.push(format!("shr {}, {value}", dst.name()));
        emit_rex(&mut self.bytes, true, None, None, Some(dst));
        self.bytes
            .extend_from_slice(&[0xC1, modrm(0b11, 5, dst.low3()), value]);
    }

    pub fn jmp(&mut self, label: Label) -> Result<(), AsmError> {
        self.emit_label_rel32(label, &[0xE9], "jmp")
    }

    pub fn jz(&mut self, label: Label) -> Result<(), AsmError> {
        self.emit_label_rel32(label, &[0x0F, 0x84], "jz")
    }

    pub fn jnz(&mut self, label: Label) -> Result<(), AsmError> {
        self.emit_label_rel32(label, &[0x0F, 0x85], "jnz")
    }

    pub fn ja(&mut self, label: Label) -> Result<(), AsmError> {
        self.emit_label_rel32(label, &[0x0F, 0x87], "ja")
    }

    pub fn call_reg(&mut self, target: Reg64) {
        self.dump.push(format!("call {}", target.name()));
        emit_rex(&mut self.bytes, false, None, None, Some(target));
        self.bytes.push(0xFF);
        self.bytes.push(modrm(0b11, 2, target.low3()));
    }

    pub fn push_reg(&mut self, reg: Reg64) {
        self.dump.push(format!("push {}", reg.name()));
        self.push_reg_raw(reg);
    }

    pub fn pop_reg(&mut self, reg: Reg64) {
        self.dump.push(format!("pop {}", reg.name()));
        self.pop_reg_raw(reg);
    }

    fn emit_label_rel32(
        &mut self,
        label: Label,
        opcode: &[u8],
        mnemonic: &str,
    ) -> Result<(), AsmError> {
        let index = label.0;
        if self.labels.get(index).is_none() {
            return Err(AsmError::InvalidLabel { label: label.0 });
        }
        self.dump.push(format!("{mnemonic} .L{}", label.0));
        self.bytes.extend_from_slice(opcode);
        let offset = self.bytes.len();
        self.bytes.extend_from_slice(&0_i32.to_le_bytes());
        let next_ip = self.bytes.len();
        self.labels[index]
            .fixups
            .push(Rel32Fixup { offset, next_ip });
        Ok(())
    }

    pub fn finish(mut self) -> Result<CodeBuffer, AsmError> {
        for (label_index, state) in self.labels.iter().enumerate() {
            let Some(target) = state.position else {
                return Err(AsmError::UnresolvedLabel { label: label_index });
            };
            for fixup in &state.fixups {
                let disp = relative_disp32(fixup.next_ip, target)?;
                self.bytes[fixup.offset..fixup.offset + 4].copy_from_slice(&disp.to_le_bytes());
            }
        }

        Ok(CodeBuffer {
            bytes: self.bytes,
            dump: self.dump.join("\n"),
            patch_sites: self.patch_sites,
        })
    }

    fn emit_reg_reg(&mut self, opcode: u8, dst: Reg64, src: Reg64) {
        self.emit_reg_reg_width(opcode, dst, src, true);
    }

    fn emit_reg_reg_width(&mut self, opcode: u8, dst: Reg64, src: Reg64, wide: bool) {
        emit_rex(&mut self.bytes, wide, Some(src), None, Some(dst));
        self.bytes.push(opcode);
        self.bytes.push(modrm(0b11, src.low3(), dst.low3()));
    }

    fn emit_reg_mem(&mut self, opcode: u8, dst: Reg64, src: Mem64) {
        self.emit_reg_mem_width(opcode, dst, src, true);
    }

    fn emit_reg_mem_width(&mut self, opcode: u8, dst: Reg64, src: Mem64, wide: bool) {
        emit_rex(&mut self.bytes, wide, Some(dst), src.index, Some(src.base));
        self.bytes.push(opcode);
        emit_mem_modrm(&mut self.bytes, dst.low3(), src);
    }

    fn emit_mem_reg(&mut self, opcode: u8, dst: Mem64, src: Reg64) {
        self.emit_mem_reg_width(opcode, dst, src, true);
    }

    fn emit_mem_reg_width(&mut self, opcode: u8, dst: Mem64, src: Reg64, wide: bool) {
        emit_rex(&mut self.bytes, wide, Some(src), dst.index, Some(dst.base));
        self.bytes.push(opcode);
        emit_mem_modrm(&mut self.bytes, src.low3(), dst);
    }

    fn emit_xmm_mem(&mut self, prefix: &[u8], opcode: u8, dst: RegXmm, src: Mem64) {
        self.bytes.extend_from_slice(prefix);
        emit_rex_xmm(&mut self.bytes, false, Some(dst), src.index, Some(src.base));
        self.bytes.push(0x0F);
        self.bytes.push(opcode);
        emit_mem_modrm(&mut self.bytes, dst.low3(), src);
    }

    fn emit_xmm_mem_0f38(&mut self, prefix: &[u8], opcode: u8, dst: RegXmm, src: Mem64) {
        self.bytes.extend_from_slice(prefix);
        emit_rex_xmm(&mut self.bytes, false, Some(dst), src.index, Some(src.base));
        self.bytes.extend_from_slice(&[0x0F, 0x38, opcode]);
        emit_mem_modrm(&mut self.bytes, dst.low3(), src);
    }

    fn emit_xmm_mem_imm8(&mut self, prefix: &[u8], opcode: u8, dst: RegXmm, src: Mem64, imm: u8) {
        self.bytes.extend_from_slice(prefix);
        emit_rex_xmm(&mut self.bytes, false, Some(dst), src.index, Some(src.base));
        self.bytes.push(0x0F);
        self.bytes.push(opcode);
        emit_mem_modrm(&mut self.bytes, dst.low3(), src);
        self.bytes.push(imm);
    }

    fn emit_mem_xmm(&mut self, prefix: &[u8], opcode: u8, dst: Mem64, src: RegXmm) {
        self.bytes.extend_from_slice(prefix);
        emit_rex_xmm(&mut self.bytes, false, Some(src), dst.index, Some(dst.base));
        self.bytes.push(0x0F);
        self.bytes.push(opcode);
        emit_mem_modrm(&mut self.bytes, src.low3(), dst);
    }

    fn emit_evex_xmm_mem(&mut self, spec: EvexSpec, opcode: u8, dst: RegXmm, src: Mem64) {
        self.emit_evex_prefix(
            spec,
            dst.number(),
            src.index.map(Reg64::number),
            src.base.number(),
        );
        self.bytes.push(opcode);
        emit_mem_modrm(&mut self.bytes, dst.low3(), src);
    }

    fn emit_evex_mem_xmm(&mut self, spec: EvexSpec, opcode: u8, dst: Mem64, src: RegXmm) {
        self.emit_evex_prefix(
            spec,
            src.number(),
            dst.index.map(Reg64::number),
            dst.base.number(),
        );
        self.bytes.push(opcode);
        emit_mem_modrm(&mut self.bytes, src.low3(), dst);
    }

    fn emit_evex_prefix(
        &mut self,
        spec: EvexSpec,
        reg_number: u8,
        index_number: Option<u8>,
        rm_number: u8,
    ) {
        self.bytes.push(0x62);
        self.bytes.push(evex_p0(
            spec.map,
            reg_number,
            index_number.unwrap_or(0),
            rm_number,
        ));
        self.bytes.push(evex_p1(spec.pp, spec.vvvv, spec.wide));
        self.bytes.push(0x08);
    }

    fn emit_reg_imm32(&mut self, opcode_extension: u8, dst: Reg64, value: i32) {
        emit_rex(&mut self.bytes, true, None, None, Some(dst));
        self.bytes.push(0x81);
        self.bytes
            .push(modrm(0b11, opcode_extension & 0b111, dst.low3()));
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_reg_raw(&mut self, reg: Reg64) {
        emit_rex(&mut self.bytes, false, None, None, Some(reg));
        self.bytes.push(0x50 + reg.low3());
    }

    fn pop_reg_raw(&mut self, reg: Reg64) {
        emit_rex(&mut self.bytes, false, None, None, Some(reg));
        self.bytes.push(0x58 + reg.low3());
    }
}

#[derive(Clone, Copy)]
struct EvexSpec {
    map: u8,
    pp: u8,
    wide: bool,
    vvvv: Option<RegXmm>,
}

impl EvexSpec {
    const fn map_0f(pp: u8, vvvv: Option<RegXmm>) -> Self {
        Self {
            map: 0x01,
            pp,
            wide: true,
            vvvv,
        }
    }

    const fn map_0f38(pp: u8, vvvv: Option<RegXmm>) -> Self {
        Self {
            map: 0x02,
            pp,
            wide: true,
            vvvv,
        }
    }
}

fn relative_disp32(source_next_ip: usize, target: usize) -> Result<i32, AsmError> {
    let disp = target as i128 - source_next_ip as i128;
    i32::try_from(disp).map_err(|_| AsmError::RelativeOutOfRange {
        source_ip: source_next_ip,
        target,
    })
}

fn emit_rex(
    bytes: &mut Vec<u8>,
    w: bool,
    reg: Option<Reg64>,
    index: Option<Reg64>,
    rm: Option<Reg64>,
) {
    let rex = 0x40
        | (u8::from(w) << 3)
        | (u8::from(reg.is_some_and(Reg64::rex_bit)) << 2)
        | (u8::from(index.is_some_and(Reg64::rex_bit)) << 1)
        | u8::from(rm.is_some_and(Reg64::rex_bit));
    if rex != 0x40 {
        bytes.push(rex);
    }
}

fn emit_rex_xmm(
    bytes: &mut Vec<u8>,
    w: bool,
    reg: Option<RegXmm>,
    index: Option<Reg64>,
    rm: Option<Reg64>,
) {
    let rex = 0x40
        | (u8::from(w) << 3)
        | (u8::from(reg.is_some_and(RegXmm::rex_bit)) << 2)
        | (u8::from(index.is_some_and(Reg64::rex_bit)) << 1)
        | u8::from(rm.is_some_and(Reg64::rex_bit));
    if rex != 0x40 {
        bytes.push(rex);
    }
}

fn evex_p0(map: u8, reg_number: u8, index_number: u8, rm_number: u8) -> u8 {
    (u8::from(reg_number & 0b1000 == 0) << 7)
        | (u8::from(index_number & 0b1000 == 0) << 6)
        | (u8::from(rm_number & 0b1000 == 0) << 5)
        | (1 << 4)
        | (map & 0x0F)
}

fn evex_p1(pp: u8, vvvv: Option<RegXmm>, wide: bool) -> u8 {
    let inverted_vvvv = vvvv.map_or(0x0F, |reg| (!reg.number()) & 0x0F);
    (u8::from(wide) << 7) | (inverted_vvvv << 3) | 0x04 | (pp & 0x03)
}

const fn modrm(mode: u8, reg: u8, rm: u8) -> u8 {
    ((mode & 0b11) << 6) | ((reg & 0b111) << 3) | (rm & 0b111)
}

fn emit_mem_modrm(bytes: &mut Vec<u8>, reg_field: u8, mem: Mem64) {
    let base = mem.base;
    let base_low = base.low3();
    let needs_sib = mem.index.is_some() || matches!(base, Reg64::Rsp | Reg64::R12);
    let force_disp8_zero = mem.disp == 0 && matches!(base, Reg64::Rbp | Reg64::R13);
    let displacement = if mem.disp == 0 && !force_disp8_zero {
        Displacement::None
    } else if force_disp8_zero {
        Displacement::I8(0)
    } else if let Ok(disp) = i8::try_from(mem.disp) {
        Displacement::I8(disp)
    } else {
        Displacement::I32(mem.disp)
    };
    let mode = displacement.mode();

    let rm = if needs_sib { 0b100 } else { base_low };
    bytes.push(modrm(mode, reg_field, rm));
    if needs_sib {
        let index = mem.index.map_or(0b100, Reg64::low3);
        bytes.push((index << 3) | base_low);
    }

    match displacement {
        Displacement::None => {}
        Displacement::I8(value) => bytes.push(value.to_le_bytes()[0]),
        Displacement::I32(value) => bytes.extend_from_slice(&value.to_le_bytes()),
    }
}

enum Displacement {
    None,
    I8(i8),
    I32(i32),
}

impl Displacement {
    const fn mode(&self) -> u8 {
        match self {
            Self::None => 0b00,
            Self::I8(_) => 0b01,
            Self::I32(_) => 0b10,
        }
    }
}

fn mem_name(mem: Mem64) -> String {
    let index = mem
        .index
        .map_or_else(String::new, |index| format!("+{}", index.name()));
    match mem.disp.cmp(&0) {
        std::cmp::Ordering::Equal => format!("[{}{index}]", mem.base.name()),
        std::cmp::Ordering::Greater => {
            format!("[{}{index}+{:#x}]", mem.base.name(), mem.disp)
        }
        std::cmp::Ordering::Less => {
            let disp = i64::from(mem.disp).abs();
            format!("[{}{index}-{disp:#x}]", mem.base.name())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AsmError, Assembler, CHAIN_EXIT_SIZE, Mem64, PatchKind, Reg64, RegMask, RegXmm,
        encode_jmp_rel32,
    };

    #[test]
    fn emits_basic_integer_bytes() {
        let mut asm = Assembler::new();

        asm.mov_reg_imm64(Reg64::Rax, 0x1122_3344_5566_7788);
        asm.mov_reg_reg(Reg64::Rcx, Reg64::Rax);
        asm.add_reg_reg(Reg64::Rax, Reg64::Rcx);
        asm.sub_reg_reg(Reg64::Rax, Reg64::Rcx);
        asm.cmp_reg_reg(Reg64::Rax, Reg64::Rcx);
        asm.ret();

        let code = asm.finish().expect("assembler should finish");
        assert_eq!(
            code.bytes(),
            &[
                0x48, 0xB8, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x48, 0x89, 0xC1, 0x48,
                0x01, 0xC8, 0x48, 0x29, 0xC8, 0x48, 0x39, 0xC8, 0xC3,
            ]
        );
        assert!(code.dump().contains("mov rax, 0x1122334455667788"));
        assert!(code.dump().contains("cmp rax, rcx"));
    }

    #[test]
    fn emits_logical_bytes() {
        let mut asm = Assembler::new();

        asm.and_reg_reg(Reg64::Rax, Reg64::Rcx);
        asm.or_reg_reg(Reg64::Rax, Reg64::Rcx);
        asm.xor_reg_reg(Reg64::Rax, Reg64::Rcx);

        let code = asm.finish().expect("assembler should finish");
        assert_eq!(
            code.bytes(),
            &[0x48, 0x21, 0xC8, 0x48, 0x09, 0xC8, 0x48, 0x31, 0xC8]
        );
        assert!(code.dump().contains("and rax, rcx"));
        assert!(code.dump().contains("xor rax, rcx"));
    }

    #[test]
    fn emits_extended_register_bytes() {
        let mut asm = Assembler::new();

        asm.mov_reg_imm64(Reg64::R8, 0xAABB);
        asm.mov_reg_reg(Reg64::R9, Reg64::R8);
        asm.add_reg_imm32(Reg64::R9, 7);
        asm.sub_reg_imm32(Reg64::R9, 3);

        let code = asm.finish().expect("assembler should finish");
        assert_eq!(
            code.bytes(),
            &[
                0x49, 0xB8, 0xBB, 0xAA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4D, 0x89, 0xC1, 0x49,
                0x81, 0xC1, 0x07, 0x00, 0x00, 0x00, 0x49, 0x81, 0xE9, 0x03, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn emits_memory_operand_bytes() {
        let mut asm = Assembler::new();

        asm.mov_mem_reg(Mem64::new(Reg64::Rdi, 256), Reg64::Rax);
        asm.mov_reg_mem(Reg64::Rax, Mem64::new(Reg64::Rdi, 256));
        asm.mov_mem_reg(Mem64::new(Reg64::Rbp, -8), Reg64::Rax);
        asm.mov_reg_mem(Reg64::Rax, Mem64::new(Reg64::Rsp, 16));

        let code = asm.finish().expect("assembler should finish");
        assert_eq!(
            code.bytes(),
            &[
                0x48, 0x89, 0x87, 0x00, 0x01, 0x00, 0x00, 0x48, 0x8B, 0x87, 0x00, 0x01, 0x00, 0x00,
                0x48, 0x89, 0x45, 0xF8, 0x48, 0x8B, 0x44, 0x24, 0x10,
            ]
        );
    }

    #[test]
    fn emits_indexed_and_width_specific_memory_bytes() {
        let mut asm = Assembler::new();

        asm.mov_reg_mem(Reg64::Rax, Mem64::indexed(Reg64::R14, Reg64::Rcx, 0));
        asm.mov_mem_reg(Mem64::indexed(Reg64::R14, Reg64::Rax, 0), Reg64::Rcx);
        asm.mov_reg_mem32(Reg64::Rax, Mem64::indexed(Reg64::R14, Reg64::Rcx, 0));
        asm.movzx_reg_mem8(Reg64::Rcx, Mem64::indexed(Reg64::R13, Reg64::Rcx, 0));

        let code = asm.finish().expect("assembler should finish");
        assert_eq!(
            code.bytes(),
            &[
                0x49, 0x8B, 0x04, 0x0E, 0x49, 0x89, 0x0C, 0x06, 0x41, 0x8B, 0x04, 0x0E, 0x49, 0x0F,
                0xB6, 0x4C, 0x0D, 0x00,
            ]
        );
    }

    #[test]
    fn emits_scalar_double_sse_memory_ops() {
        let mut asm = Assembler::new();

        asm.movsd_xmm_mem(RegXmm::Xmm0, Mem64::new(Reg64::R15, 336));
        asm.addsd_xmm_mem(RegXmm::Xmm0, Mem64::new(Reg64::R15, 352));
        asm.subsd_xmm_mem(RegXmm::Xmm0, Mem64::new(Reg64::R15, 352));
        asm.mulsd_xmm_mem(RegXmm::Xmm0, Mem64::new(Reg64::R15, 352));
        asm.divsd_xmm_mem(RegXmm::Xmm0, Mem64::new(Reg64::R15, 352));
        asm.movsd_mem_xmm(Mem64::new(Reg64::R15, 368), RegXmm::Xmm0);

        let code = asm.finish().expect("assembler should finish");
        assert!(code.dump().contains("addsd xmm0"));
        assert!(code.dump().contains("movsd qword [r15+0x170], xmm0"));
        assert_eq!(
            &code.bytes()[0..8],
            &[0xF2, 0x41, 0x0F, 0x10, 0x87, 0x50, 0x01, 0x00]
        );
    }

    #[test]
    fn emits_packed_vector_memory_ops() {
        let mut asm = Assembler::new();

        asm.movdqu_xmm_mem(RegXmm::Xmm1, Mem64::new(Reg64::R15, 336));
        asm.paddq_xmm_mem(RegXmm::Xmm1, Mem64::new(Reg64::R15, 352));
        asm.addpd_xmm_mem(RegXmm::Xmm1, Mem64::new(Reg64::R15, 352));
        asm.pcmpeqq_xmm_mem(RegXmm::Xmm1, Mem64::new(Reg64::R15, 352));
        asm.pshufd_xmm_mem_imm8(RegXmm::Xmm1, Mem64::new(Reg64::R15, 336), 0x4e);
        asm.movdqu_mem_xmm(Mem64::new(Reg64::R15, 368), RegXmm::Xmm1);

        let code = asm.finish().expect("assembler should finish");
        assert!(code.dump().contains("movdqu xmm1"));
        assert!(code.dump().contains("paddq xmm1"));
        assert!(code.dump().contains("pcmpeqq xmm1"));
        assert!(code.dump().contains("pshufd xmm1"));
        assert_eq!(
            &code.bytes()[0..8],
            &[0xF3, 0x41, 0x0F, 0x6F, 0x8F, 0x50, 0x01, 0x00]
        );
    }

    #[test]
    fn emits_avx512_mask_compare_bytes() {
        let mut asm = Assembler::new();

        asm.vmovdqu64_xmm_mem(RegXmm::Xmm0, Mem64::new(Reg64::R15, 336));
        asm.vmovdqu64_xmm_mem(RegXmm::Xmm1, Mem64::new(Reg64::R15, 352));
        asm.vpcmpeqq_mask_xmm_xmm(RegMask::K1, RegXmm::Xmm0, RegXmm::Xmm1);
        asm.vpmovm2q_xmm_mask(RegXmm::Xmm0, RegMask::K1);
        asm.vmovdqu64_mem_xmm(Mem64::new(Reg64::R15, 368), RegXmm::Xmm0);

        let code = asm.finish().expect("assembler should finish");
        assert!(code.dump().contains("vpcmpeqq k1, xmm0, xmm1"));
        assert!(code.dump().contains("vpmovm2q xmm0, k1"));
        assert_eq!(
            &code.bytes()[0..9],
            &[0x62, 0xD1, 0xFE, 0x08, 0x6F, 0x87, 0x50, 0x01, 0x00]
        );
        assert!(
            code.bytes()
                .windows(6)
                .any(|bytes| bytes == [0x62, 0xF2, 0xFD, 0x08, 0x29, 0xC9])
        );
        assert!(
            code.bytes()
                .windows(6)
                .any(|bytes| bytes == [0x62, 0xF2, 0xFE, 0x08, 0x38, 0xC1])
        );
    }

    #[test]
    fn patches_forward_label() {
        let mut asm = Assembler::new();
        let label = asm.create_label();

        asm.jmp(label).expect("label exists");
        asm.ret();
        asm.bind_label(label).expect("label should bind");
        asm.ret();

        let code = asm.finish().expect("assembler should finish");
        assert_eq!(code.bytes(), &[0xE9, 0x01, 0x00, 0x00, 0x00, 0xC3, 0xC3]);
        assert!(code.dump().contains(".L0:"));
    }

    #[test]
    fn rejects_unresolved_label() {
        let mut asm = Assembler::new();
        let label = asm.create_label();

        asm.jmp(label).expect("label exists");

        let error = asm.finish().expect_err("label should be unresolved");
        assert_eq!(error, AsmError::UnresolvedLabel { label: 0 });
    }

    #[test]
    fn emits_stack_frame_helpers() {
        let mut asm = Assembler::new();

        asm.prologue();
        asm.sub_reg_imm32(Reg64::Rsp, 32);
        asm.add_reg_imm32(Reg64::Rsp, 32);
        asm.epilogue();

        let code = asm.finish().expect("assembler should finish");
        assert_eq!(
            code.bytes(),
            &[
                0x55, 0x48, 0x89, 0xE5, 0x48, 0x81, 0xEC, 0x20, 0x00, 0x00, 0x00, 0x48, 0x81, 0xC4,
                0x20, 0x00, 0x00, 0x00, 0x5D, 0xC3,
            ]
        );
    }

    #[test]
    fn chain_epilogue_emits_pop_and_patchable_slot() {
        let mut asm = Assembler::new();
        asm.prologue();
        asm.chain_epilogue();
        let code = asm.finish().expect("assembler should finish");

        // push rbp; mov rbp,rsp; pop rbp; (chain slot: ret + 4 nop)
        assert_eq!(
            code.bytes(),
            &[0x55, 0x48, 0x89, 0xE5, 0x5D, 0xC3, 0x90, 0x90, 0x90, 0x90]
        );

        let sites = code.patch_sites();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, PatchKind::ChainExit);
        assert_eq!(sites[0].size, CHAIN_EXIT_SIZE);
        // The slot begins at the `ret` byte (offset 5), right after `pop rbp`.
        assert_eq!(sites[0].offset, 5);
        assert_eq!(code.bytes()[sites[0].offset], 0xC3);
    }

    #[test]
    fn jmp_rel32_encoding_is_correct() {
        // jmp rel32 = E9 + little-endian displacement.
        assert_eq!(encode_jmp_rel32(0), [0xE9, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(encode_jmp_rel32(5), [0xE9, 0x05, 0x00, 0x00, 0x00]);
        assert_eq!(encode_jmp_rel32(-2), [0xE9, 0xFE, 0xFF, 0xFF, 0xFF]);
    }
}
