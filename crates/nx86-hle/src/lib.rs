pub const CRATE_NAME: &str = "nx86-hle";

use nx86_service::GuestServiceName;

pub const SERVICE_STATUS_SUCCESS: u64 = 0;
pub const SYNTHETIC_PAGE_SIZE: u64 = 4096;
pub const NEUTRAL_INPUT_STATE: u64 = 0;

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServiceDispatcher;

impl ServiceDispatcher {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    #[must_use]
    pub const fn handle_svc(self, request: SvcRequest) -> SvcOutcome {
        match request.imm {
            0 => SvcOutcome::Exit { code: request.x(0) },
            1 => SvcOutcome::Continue {
                service: ServiceName::FileSystem,
                status: SERVICE_STATUS_SUCCESS,
                value: 0,
            },
            2 => SvcOutcome::Continue {
                service: ServiceName::Thread,
                status: SERVICE_STATUS_SUCCESS,
                value: request.thread_id,
            },
            3 => SvcOutcome::Continue {
                service: ServiceName::Memory,
                status: SERVICE_STATUS_SUCCESS,
                value: SYNTHETIC_PAGE_SIZE,
            },
            4 => SvcOutcome::Continue {
                service: ServiceName::Input,
                status: SERVICE_STATUS_SUCCESS,
                value: request.input_state,
            },
            5 | 6 => SvcOutcome::Continue {
                service: ServiceName::AudioOut,
                status: request.audio_status,
                value: request.audio_value,
            },
            _ => SvcOutcome::Unhandled { imm: request.imm },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MinimalServiceTable {
    dispatcher: ServiceDispatcher,
}

impl Default for MinimalServiceTable {
    fn default() -> Self {
        Self::new()
    }
}

impl MinimalServiceTable {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            dispatcher: ServiceDispatcher::new(),
        }
    }

    #[must_use]
    pub const fn handle_svc(self, imm: u16, x0: u64) -> SvcOutcome {
        self.dispatcher.handle_svc(SvcRequest::from_x0(imm, x0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SvcRequest {
    pub imm: u16,
    pub args: [u64; 8],
    pub thread_id: u64,
    pub input_state: u64,
    pub audio_status: u64,
    pub audio_value: u64,
}

impl SvcRequest {
    #[must_use]
    pub const fn new(imm: u16, args: [u64; 8], thread_id: u64, input_state: u64) -> Self {
        Self {
            imm,
            args,
            thread_id,
            input_state,
            audio_status: SERVICE_STATUS_SUCCESS,
            audio_value: 0,
        }
    }

    #[must_use]
    pub const fn from_x0(imm: u16, x0: u64) -> Self {
        let mut args = [0; 8];
        args[0] = x0;
        Self {
            imm,
            args,
            thread_id: 1,
            input_state: NEUTRAL_INPUT_STATE,
            audio_status: SERVICE_STATUS_SUCCESS,
            audio_value: 0,
        }
    }

    #[must_use]
    pub const fn with_audio_result(mut self, status: u64, value: u64) -> Self {
        self.audio_status = status;
        self.audio_value = value;
        self
    }

    #[must_use]
    pub const fn x(self, index: usize) -> u64 {
        if index < self.args.len() {
            self.args[index]
        } else {
            0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceName {
    FileSystem,
    Thread,
    Memory,
    Input,
    AudioOut,
}

impl ServiceName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileSystem => "filesystem",
            Self::Thread => "thread",
            Self::Memory => "memory",
            Self::Input => "input",
            Self::AudioOut => "audout:u",
        }
    }

    #[must_use]
    pub const fn guest_service(self) -> GuestServiceName {
        match self {
            Self::FileSystem => GuestServiceName::FileSystem,
            Self::Thread => GuestServiceName::Thread,
            Self::Memory => GuestServiceName::Memory,
            Self::Input => GuestServiceName::Input,
            Self::AudioOut => GuestServiceName::AudioOut,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvcOutcome {
    Continue {
        service: ServiceName,
        status: u64,
        value: u64,
    },
    Exit {
        code: u64,
    },
    Unhandled {
        imm: u16,
    },
}

impl SvcOutcome {
    #[must_use]
    pub const fn is_clean_exit(self) -> bool {
        matches!(self, Self::Exit { .. })
    }

    #[must_use]
    pub const fn service(self) -> Option<ServiceName> {
        match self {
            Self::Continue { service, .. } => Some(service),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NEUTRAL_INPUT_STATE, SERVICE_STATUS_SUCCESS, SYNTHETIC_PAGE_SIZE, ServiceDispatcher,
        ServiceName, SvcOutcome, SvcRequest,
    };

    #[test]
    fn svc_zero_is_homebrew_exit() {
        let services = ServiceDispatcher::new();

        assert_eq!(
            services.handle_svc(SvcRequest::from_x0(0, 42)),
            SvcOutcome::Exit { code: 42 }
        );
    }

    #[test]
    fn filesystem_service_returns_success_stub() {
        let services = ServiceDispatcher::new();

        assert_eq!(
            services.handle_svc(SvcRequest::from_x0(1, 0)),
            SvcOutcome::Continue {
                service: ServiceName::FileSystem,
                status: SERVICE_STATUS_SUCCESS,
                value: 0,
            }
        );
    }

    #[test]
    fn thread_service_reports_current_synthetic_thread() {
        let services = ServiceDispatcher::new();
        let request = SvcRequest::new(2, [0; 8], 9, NEUTRAL_INPUT_STATE);

        assert_eq!(
            services.handle_svc(request),
            SvcOutcome::Continue {
                service: ServiceName::Thread,
                status: SERVICE_STATUS_SUCCESS,
                value: 9,
            }
        );
    }

    #[test]
    fn memory_service_reports_synthetic_page_size() {
        let services = ServiceDispatcher::new();

        assert_eq!(
            services.handle_svc(SvcRequest::from_x0(3, 0)),
            SvcOutcome::Continue {
                service: ServiceName::Memory,
                status: SERVICE_STATUS_SUCCESS,
                value: SYNTHETIC_PAGE_SIZE,
            }
        );
    }

    #[test]
    fn input_service_returns_neutral_controller_state() {
        let services = ServiceDispatcher::new();

        assert_eq!(
            services.handle_svc(SvcRequest::from_x0(4, 0)),
            SvcOutcome::Continue {
                service: ServiceName::Input,
                status: SERVICE_STATUS_SUCCESS,
                value: NEUTRAL_INPUT_STATE,
            }
        );
    }

    #[test]
    fn input_service_returns_request_controller_state() {
        let services = ServiceDispatcher::new();
        let request = SvcRequest::new(4, [0; 8], 1, 0x4010);

        assert_eq!(
            services.handle_svc(request),
            SvcOutcome::Continue {
                service: ServiceName::Input,
                status: SERVICE_STATUS_SUCCESS,
                value: 0x4010,
            }
        );
    }

    #[test]
    fn audio_service_returns_runtime_supplied_status_and_value() {
        let services = ServiceDispatcher::new();
        let request = SvcRequest::from_x0(6, 0).with_audio_result(0, 128);

        assert_eq!(
            services.handle_svc(request),
            SvcOutcome::Continue {
                service: ServiceName::AudioOut,
                status: SERVICE_STATUS_SUCCESS,
                value: 128,
            }
        );
        assert_eq!(ServiceName::AudioOut.as_str(), "audout:u");
        assert_eq!(
            ServiceName::AudioOut.guest_service(),
            nx86_service::GuestServiceName::AudioOut
        );
    }

    #[test]
    fn unknown_svc_is_reported_for_later_service_dispatch() {
        let services = ServiceDispatcher::new();

        assert_eq!(
            services.handle_svc(SvcRequest::from_x0(7, 0)),
            SvcOutcome::Unhandled { imm: 7 }
        );
    }
}
