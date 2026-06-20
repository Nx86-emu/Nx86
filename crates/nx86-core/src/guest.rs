use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct CpuState {
    general: [u64; 31],
    sp: u64,
    pc: u64,
    nzcv: Nzcv,
    fp_simd: [u128; 32],
    fpcr: u32,
    fpsr: u32,
    thread: ThreadState,
    halted: bool,
    halt_reason: Option<String>,
}

impl CpuState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn general_registers(&self) -> &[u64; 31] {
        &self.general
    }

    #[must_use]
    pub fn x(&self, register: u8) -> u64 {
        if register < 31 {
            self.general[usize::from(register)]
        } else {
            0
        }
    }

    pub fn set_x(&mut self, register: u8, value: u64) {
        if register < 31 {
            self.general[usize::from(register)] = value;
        }
    }

    #[must_use]
    pub fn read_gp_or_sp(&self, register: u8) -> u64 {
        if register == 31 {
            self.sp
        } else {
            self.x(register)
        }
    }

    pub fn write_gp_or_sp(&mut self, register: u8, value: u64) {
        if register == 31 {
            self.sp = value;
        } else {
            self.set_x(register, value);
        }
    }

    #[must_use]
    pub const fn sp(&self) -> u64 {
        self.sp
    }

    pub const fn set_sp(&mut self, value: u64) {
        self.sp = value;
    }

    #[must_use]
    pub const fn pc(&self) -> u64 {
        self.pc
    }

    pub const fn set_pc(&mut self, value: u64) {
        self.pc = value;
    }

    #[must_use]
    pub const fn nzcv(&self) -> Nzcv {
        self.nzcv
    }

    pub const fn set_nzcv(&mut self, value: Nzcv) {
        self.nzcv = value;
    }

    #[must_use]
    pub const fn fpcr(&self) -> u32 {
        self.fpcr
    }

    pub const fn set_fpcr(&mut self, value: u32) {
        self.fpcr = value;
    }

    #[must_use]
    pub const fn fpsr(&self) -> u32 {
        self.fpsr
    }

    pub const fn set_fpsr(&mut self, value: u32) {
        self.fpsr = value;
    }

    #[must_use]
    pub const fn vector(&self, register: u8) -> u128 {
        if register < 32 {
            self.fp_simd[register as usize]
        } else {
            0
        }
    }

    pub const fn set_vector(&mut self, register: u8, value: u128) {
        if register < 32 {
            self.fp_simd[register as usize] = value;
        }
    }

    #[must_use]
    pub const fn thread(&self) -> &ThreadState {
        &self.thread
    }

    pub fn set_thread(&mut self, thread: ThreadState) {
        self.thread = thread;
    }

    #[must_use]
    pub const fn halted(&self) -> bool {
        self.halted
    }

    #[must_use]
    pub fn halt_reason(&self) -> Option<&str> {
        self.halt_reason.as_deref()
    }

    pub fn halt(&mut self, reason: impl Into<String>) {
        self.halted = true;
        self.halt_reason = Some(reason.into());
    }

    pub fn clear_halt(&mut self) {
        self.halted = false;
        self.halt_reason = None;
    }

    pub fn read_register(&self, register: RegisterName) -> RegisterValue {
        match register {
            RegisterName::General(index) => RegisterValue::U64(self.x(index)),
            RegisterName::Vector(index) => RegisterValue::U128(self.vector(index)),
            RegisterName::Sp => RegisterValue::U64(self.sp),
            RegisterName::Pc => RegisterValue::U64(self.pc),
            RegisterName::Nzcv => RegisterValue::U64(u64::from(self.nzcv.bits())),
            RegisterName::Fpcr => RegisterValue::U64(u64::from(self.fpcr)),
            RegisterName::Fpsr => RegisterValue::U64(u64::from(self.fpsr)),
            RegisterName::Halted => RegisterValue::Bool(self.halted),
        }
    }

    pub fn compare_expected_registers<I>(&self, expected: I) -> Vec<CpuStateDiff>
    where
        I: IntoIterator<Item = (RegisterName, RegisterValue)>,
    {
        expected
            .into_iter()
            .filter_map(|(register, expected)| {
                let actual = self.read_register(register);
                (actual != expected).then_some(CpuStateDiff {
                    register,
                    expected,
                    actual,
                })
            })
            .collect()
    }

    #[must_use]
    pub fn dump(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("pc={:#018x} sp={:#018x}\n", self.pc, self.sp));
        output.push_str(&format!(
            "nzcv={:#010x} halted={}\n",
            self.nzcv.bits(),
            self.halted
        ));
        for chunk in 0..7 {
            let base = chunk * 4;
            output.push_str(&format!(
                "x{base:02}={:#018x} x{:02}={:#018x} x{:02}={:#018x} x{:02}={:#018x}\n",
                self.general[base],
                base + 1,
                self.general[base + 1],
                base + 2,
                self.general[base + 2],
                base + 3,
                self.general[base + 3],
            ));
        }
        output.push_str(&format!(
            "x28={:#018x} x29={:#018x} x30={:#018x}\n",
            self.general[28], self.general[29], self.general[30],
        ));
        output
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct Nzcv {
    pub negative: bool,
    pub zero: bool,
    pub carry: bool,
    pub overflow: bool,
}

impl Nzcv {
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self {
            negative: bits & (1 << 31) != 0,
            zero: bits & (1 << 30) != 0,
            carry: bits & (1 << 29) != 0,
            overflow: bits & (1 << 28) != 0,
        }
    }

    #[must_use]
    pub const fn bits(self) -> u32 {
        ((self.negative as u32) << 31)
            | ((self.zero as u32) << 30)
            | ((self.carry as u32) << 29)
            | ((self.overflow as u32) << 28)
    }

    /// Compute NZCV for a 64-bit `ADDS lhs, rhs`.
    #[must_use]
    pub fn from_add(lhs: u64, rhs: u64) -> Self {
        let (result, carry) = lhs.overflowing_add(rhs);
        let overflow = ((lhs ^ result) & (rhs ^ result)) >> 63 != 0;
        Self {
            negative: (result >> 63) != 0,
            zero: result == 0,
            carry,
            overflow,
        }
    }

    /// Compute NZCV for a 64-bit `SUBS lhs, rhs`. ARM defines the carry flag as
    /// the inverse of the unsigned borrow.
    #[must_use]
    pub fn from_sub(lhs: u64, rhs: u64) -> Self {
        let (result, borrow) = lhs.overflowing_sub(rhs);
        let overflow = ((lhs ^ rhs) & (lhs ^ result)) >> 63 != 0;
        Self {
            negative: (result >> 63) != 0,
            zero: result == 0,
            carry: !borrow,
            overflow,
        }
    }

    /// Whether these flags satisfy an AArch64 condition code.
    #[must_use]
    pub const fn satisfies(self, cond: Cond) -> bool {
        match cond {
            Cond::Eq => self.zero,
            Cond::Ne => !self.zero,
            Cond::Cs => self.carry,
            Cond::Cc => !self.carry,
            Cond::Mi => self.negative,
            Cond::Pl => !self.negative,
            Cond::Vs => self.overflow,
            Cond::Vc => !self.overflow,
            Cond::Hi => self.carry && !self.zero,
            Cond::Ls => !self.carry || self.zero,
            Cond::Ge => self.negative == self.overflow,
            Cond::Lt => self.negative != self.overflow,
            Cond::Gt => !self.zero && (self.negative == self.overflow),
            Cond::Le => self.zero || (self.negative != self.overflow),
            Cond::Al => true,
        }
    }
}

/// An AArch64 condition code.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Cond {
    Eq,
    Ne,
    Cs,
    Cc,
    Mi,
    Pl,
    Vs,
    Vc,
    Hi,
    Ls,
    Ge,
    Lt,
    Gt,
    Le,
    Al,
}

impl Cond {
    /// Decode the 4-bit condition field (`0b1111` "never" is treated as `Al`).
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 0xF {
            0 => Self::Eq,
            1 => Self::Ne,
            2 => Self::Cs,
            3 => Self::Cc,
            4 => Self::Mi,
            5 => Self::Pl,
            6 => Self::Vs,
            7 => Self::Vc,
            8 => Self::Hi,
            9 => Self::Ls,
            10 => Self::Ge,
            11 => Self::Lt,
            12 => Self::Gt,
            13 => Self::Le,
            _ => Self::Al,
        }
    }

    /// The mnemonic suffix, e.g. `eq`.
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::Cs => "cs",
            Self::Cc => "cc",
            Self::Mi => "mi",
            Self::Pl => "pl",
            Self::Vs => "vs",
            Self::Vc => "vc",
            Self::Hi => "hi",
            Self::Ls => "ls",
            Self::Ge => "ge",
            Self::Lt => "lt",
            Self::Gt => "gt",
            Self::Le => "le",
            Self::Al => "al",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ThreadState {
    pub thread_id: u64,
    pub name: Option<String>,
    pub deterministic_index: u64,
}

impl Default for ThreadState {
    fn default() -> Self {
        Self {
            thread_id: 1,
            name: Some("main".to_owned()),
            deterministic_index: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RegisterName {
    General(u8),
    Vector(u8),
    Sp,
    Pc,
    Nzcv,
    Fpcr,
    Fpsr,
    Halted,
}

impl fmt::Display for RegisterName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::General(index) => write!(formatter, "x{index}"),
            Self::Vector(index) => write!(formatter, "v{index}"),
            Self::Sp => formatter.write_str("sp"),
            Self::Pc => formatter.write_str("pc"),
            Self::Nzcv => formatter.write_str("nzcv"),
            Self::Fpcr => formatter.write_str("fpcr"),
            Self::Fpsr => formatter.write_str("fpsr"),
            Self::Halted => formatter.write_str("halted"),
        }
    }
}

impl FromStr for RegisterName {
    type Err = RegisterParseError;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        let normalized = source.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "sp" => return Ok(Self::Sp),
            "pc" => return Ok(Self::Pc),
            "nzcv" => return Ok(Self::Nzcv),
            "fpcr" => return Ok(Self::Fpcr),
            "fpsr" => return Ok(Self::Fpsr),
            "halted" => return Ok(Self::Halted),
            _ => {}
        }

        if let Some(index) = normalized.strip_prefix('x') {
            let index = parse_register_index(index, 30, source)?;
            return Ok(Self::General(index));
        }

        if let Some(index) = normalized.strip_prefix('v') {
            let index = parse_register_index(index, 31, source)?;
            return Ok(Self::Vector(index));
        }

        Err(RegisterParseError::UnknownRegister {
            name: source.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RegisterValue {
    U64(u64),
    U128(u128),
    Bool(bool),
}

impl RegisterValue {
    pub fn parse_for_register(
        register: RegisterName,
        source: &str,
    ) -> Result<Self, RegisterParseError> {
        match register {
            RegisterName::Vector(_) => parse_int_u128(source).map(Self::U128),
            RegisterName::Halted => parse_bool(source).map(Self::Bool),
            _ => parse_int_u64(source).map(Self::U64),
        }
    }
}

impl fmt::Display for RegisterValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::U64(value) => write!(formatter, "{value:#x}"),
            Self::U128(value) => write!(formatter, "{value:#x}"),
            Self::Bool(value) => write!(formatter, "{value}"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct CpuStateDiff {
    pub register: RegisterName,
    pub expected: RegisterValue,
    pub actual: RegisterValue,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RegisterParseError {
    #[error("unknown register `{name}`")]
    UnknownRegister { name: String },
    #[error("register `{name}` is out of range, max is {max}")]
    RegisterOutOfRange { name: String, max: u8 },
    #[error("invalid register value `{value}`")]
    InvalidValue { value: String },
}

fn parse_register_index(source: &str, max: u8, original: &str) -> Result<u8, RegisterParseError> {
    let index: u8 = source
        .parse()
        .map_err(|_| RegisterParseError::UnknownRegister {
            name: original.to_owned(),
        })?;

    if index > max {
        Err(RegisterParseError::RegisterOutOfRange {
            name: original.to_owned(),
            max,
        })
    } else {
        Ok(index)
    }
}

fn parse_bool(source: &str) -> Result<bool, RegisterParseError> {
    match source.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(RegisterParseError::InvalidValue {
            value: source.to_owned(),
        }),
    }
}

fn parse_int_u64(source: &str) -> Result<u64, RegisterParseError> {
    parse_int_u128(source).and_then(|value| {
        u64::try_from(value).map_err(|_| RegisterParseError::InvalidValue {
            value: source.to_owned(),
        })
    })
}

fn parse_int_u128(source: &str) -> Result<u128, RegisterParseError> {
    let trimmed = source.trim().replace('_', "");
    let parsed = if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u128::from_str_radix(hex, 16)
    } else {
        trimmed.parse()
    };

    parsed.map_err(|_| RegisterParseError::InvalidValue {
        value: source.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{CpuState, Nzcv, RegisterName, RegisterValue, ThreadState};

    #[test]
    fn cpu_state_can_be_created_and_dumped() {
        let mut state = CpuState::new();
        state.set_x(0, 7);
        state.set_sp(0x1000);
        state.set_pc(0x2000);

        let dump = state.dump();

        assert!(dump.contains("pc=0x0000000000002000"));
        assert!(dump.contains("sp=0x0000000000001000"));
        assert!(dump.contains("x00=0x0000000000000007"));
    }

    #[test]
    fn cpu_state_serializes_for_debug() {
        let mut state = CpuState::new();
        state.set_thread(ThreadState {
            thread_id: 9,
            name: Some("worker".to_owned()),
            deterministic_index: 2,
        });
        state.halt("svc #0");

        let serialized = serde_json::to_string(&state).expect("state should serialize");
        let decoded: CpuState =
            serde_json::from_str(&serialized).expect("state should deserialize");

        assert_eq!(decoded, state);
    }

    #[test]
    fn nzcv_round_trips_bits() {
        let nzcv = Nzcv::from_bits(0xA000_0000);

        assert!(nzcv.negative);
        assert!(!nzcv.zero);
        assert!(nzcv.carry);
        assert_eq!(nzcv.bits(), 0xA000_0000);
    }

    #[test]
    fn register_parser_accepts_expected_names() {
        assert_eq!(RegisterName::from_str("x30"), Ok(RegisterName::General(30)));
        assert_eq!(RegisterName::from_str("v31"), Ok(RegisterName::Vector(31)));
        assert_eq!(RegisterName::from_str("pc"), Ok(RegisterName::Pc));
    }

    #[test]
    fn expected_register_comparison_reports_mismatches() {
        let mut state = CpuState::new();
        state.set_x(0, 1);
        state.set_pc(4);

        let diffs = state.compare_expected_registers([
            (RegisterName::General(0), RegisterValue::U64(1)),
            (RegisterName::Pc, RegisterValue::U64(8)),
        ]);

        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].register, RegisterName::Pc);
        assert_eq!(diffs[0].actual, RegisterValue::U64(4));
    }

    #[test]
    fn flag_computation_matches_arm_semantics() {
        use super::Cond;

        // 1 - 1 == 0: zero set, carry set (no borrow), eq holds.
        let sub = Nzcv::from_sub(1, 1);
        assert!(sub.zero && sub.carry);
        assert!(sub.satisfies(Cond::Eq));
        assert!(!sub.satisfies(Cond::Ne));

        // 1 - 2 underflows: negative set, carry clear (borrow), lt holds.
        let borrow = Nzcv::from_sub(1, 2);
        assert!(borrow.negative && !borrow.carry);
        assert!(borrow.satisfies(Cond::Lt));
        assert!(!borrow.satisfies(Cond::Ge));

        // u64::MAX + 1 wraps to 0: zero + carry set.
        let add = Nzcv::from_add(u64::MAX, 1);
        assert!(add.zero && add.carry);

        // Signed overflow: i64::MAX + 1.
        let overflow = Nzcv::from_add(i64::MAX as u64, 1);
        assert!(overflow.overflow && overflow.negative);
    }

    #[test]
    fn condition_codes_decode_from_bits() {
        use super::Cond;

        assert_eq!(Cond::from_bits(0), Cond::Eq);
        assert_eq!(Cond::from_bits(1), Cond::Ne);
        assert_eq!(Cond::from_bits(11), Cond::Lt);
        assert_eq!(Cond::from_bits(14), Cond::Al);
        assert_eq!(Cond::from_bits(15), Cond::Al);
    }

    #[test]
    fn out_of_range_vector_access_is_defensive() {
        let mut state = CpuState::new();

        state.set_vector(255, 1);

        assert_eq!(state.vector(255), 0);
    }
}
