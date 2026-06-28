use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const IPC_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct IpcEnvelope<T> {
    pub version: u32,
    pub payload: T,
}

impl<T> IpcEnvelope<T> {
    #[must_use]
    pub const fn new(payload: T) -> Self {
        Self {
            version: IPC_VERSION,
            payload,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum IpcCommand {
    StartWorker { kind: WorkerKind, job_id: String },
    Cancel { job_id: String },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkerKind {
    CompilerSmoke,
    RuntimeSmoke,
    RebuildProfile,
}

impl WorkerKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::CompilerSmoke => "compiler-smoke",
            Self::RuntimeSmoke => "runtime-smoke",
            Self::RebuildProfile => "rebuild-profile",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum IpcEvent {
    Progress(CompileProgress),
    Cancelled(CancelledEvent),
    Log(LogEvent),
    Completed(CompletedEvent),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct CompileProgress {
    pub title_id: Option<String>,
    pub phase: String,
    pub percent: f32,
    pub current_module: Option<String>,
    pub functions_discovered: u64,
    pub functions_compiled: u64,
    pub native_coverage_estimate: f32,
    #[serde(default)]
    pub native_coverage_static: f32,
    #[serde(default)]
    pub native_coverage_executed: f32,
    #[serde(default)]
    pub fastmem_coverage: f32,
    #[serde(default)]
    pub slowmem_penalty: f32,
    /// Shader readiness percentage (SPEC §15.2 category 3), folded into
    /// `native_coverage_estimate` via the min-gate in [`crate::coverage`].
    #[serde(default)]
    pub shader_readiness: f32,
    pub cache_size_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct CancelledEvent {
    pub job_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct LogEvent {
    pub level: LogLevel,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct CompletedEvent {
    pub job_id: String,
    pub success: bool,
    pub message: String,
}

pub fn encode_event(event: &IpcEvent) -> Result<String, IpcError> {
    encode_envelope(&IpcEnvelope::new(event))
}

pub fn decode_event(line: &str) -> Result<IpcEvent, IpcError> {
    let envelope: IpcEnvelope<IpcEvent> = decode_envelope(line)?;
    Ok(envelope.payload)
}

pub fn encode_command(command: &IpcCommand) -> Result<String, IpcError> {
    encode_envelope(&IpcEnvelope::new(command))
}

pub fn decode_command(line: &str) -> Result<IpcCommand, IpcError> {
    let envelope: IpcEnvelope<IpcCommand> = decode_envelope(line)?;
    Ok(envelope.payload)
}

fn encode_envelope<T: Serialize>(envelope: &IpcEnvelope<T>) -> Result<String, IpcError> {
    let mut encoded = serde_json::to_string(envelope).map_err(IpcError::Serialize)?;
    encoded.push('\n');
    Ok(encoded)
}

fn decode_envelope<T: for<'de> Deserialize<'de>>(line: &str) -> Result<IpcEnvelope<T>, IpcError> {
    let envelope: IpcEnvelope<T> =
        serde_json::from_str(line.trim()).map_err(IpcError::Deserialize)?;
    if envelope.version == IPC_VERSION {
        Ok(envelope)
    } else {
        Err(IpcError::Version {
            expected: IPC_VERSION,
            actual: envelope.version,
        })
    }
}

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("failed to serialize IPC message: {0}")]
    Serialize(serde_json::Error),
    #[error("failed to parse IPC message: {0}")]
    Deserialize(serde_json::Error),
    #[error("unsupported IPC version {actual}, expected {expected}")]
    Version { expected: u32, actual: u32 },
}

#[cfg(test)]
mod tests {
    use super::{
        CancelledEvent, CompileProgress, IpcCommand, IpcEvent, WorkerKind, decode_command,
        decode_event, encode_command, encode_event,
    };

    #[test]
    fn event_json_round_trips() {
        let event = IpcEvent::Progress(CompileProgress {
            title_id: Some("0100ABCD12345678".to_owned()),
            phase: "discover".to_owned(),
            percent: 42.0,
            current_module: Some("main".to_owned()),
            functions_discovered: 100,
            functions_compiled: 40,
            native_coverage_estimate: 12.5,
            native_coverage_static: 10.0,
            native_coverage_executed: 20.0,
            fastmem_coverage: 90.0,
            slowmem_penalty: 10.0,
            shader_readiness: 60.0,
            cache_size_bytes: 4096,
        });

        let encoded = encode_event(&event).expect("event should encode");
        let decoded = decode_event(&encoded).expect("event should decode");

        assert_eq!(decoded, event);
    }

    #[test]
    fn cancellation_command_round_trips() {
        let command = IpcCommand::Cancel {
            job_id: "job-1".to_owned(),
        };

        let encoded = encode_command(&command).expect("command should encode");
        let decoded = decode_command(&encoded).expect("command should decode");

        assert_eq!(decoded, command);
    }

    #[test]
    fn cancelled_event_round_trips() {
        let event = IpcEvent::Cancelled(CancelledEvent {
            job_id: "job-1".to_owned(),
            reason: "user".to_owned(),
        });

        let encoded = encode_event(&event).expect("event should encode");
        let decoded = decode_event(&encoded).expect("event should decode");

        assert_eq!(decoded, event);
    }

    #[test]
    fn worker_kind_has_cli_label() {
        assert_eq!(WorkerKind::CompilerSmoke.label(), "compiler-smoke");
        assert_eq!(WorkerKind::RuntimeSmoke.label(), "runtime-smoke");
        assert_eq!(WorkerKind::RebuildProfile.label(), "rebuild-profile");
    }

    #[test]
    fn rebuild_profile_command_round_trips() {
        let command = IpcCommand::StartWorker {
            kind: WorkerKind::RebuildProfile,
            job_id: "rebuild-1".to_owned(),
        };

        let encoded = encode_command(&command).expect("command should encode");
        let decoded = decode_command(&encoded).expect("command should decode");

        assert_eq!(decoded, command);
    }
}
