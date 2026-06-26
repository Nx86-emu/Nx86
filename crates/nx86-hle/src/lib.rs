pub const CRATE_NAME: &str = "nx86-hle";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MinimalServiceTable;

impl MinimalServiceTable {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    #[must_use]
    pub const fn handle_svc(self, imm: u16, x0: u64) -> SvcOutcome {
        match imm {
            0 => SvcOutcome::Exit { code: x0 },
            _ => SvcOutcome::Unhandled { imm },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvcOutcome {
    Exit { code: u64 },
    Unhandled { imm: u16 },
}

impl SvcOutcome {
    #[must_use]
    pub const fn is_clean_exit(self) -> bool {
        matches!(self, Self::Exit { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::{MinimalServiceTable, SvcOutcome};

    #[test]
    fn svc_zero_is_homebrew_exit() {
        let services = MinimalServiceTable::new();

        assert_eq!(services.handle_svc(0, 42), SvcOutcome::Exit { code: 42 });
    }

    #[test]
    fn unknown_svc_is_reported_for_later_service_dispatch() {
        let services = MinimalServiceTable::new();

        assert_eq!(services.handle_svc(7, 0), SvcOutcome::Unhandled { imm: 7 });
    }
}
