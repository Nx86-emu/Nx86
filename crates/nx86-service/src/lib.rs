use std::collections::BTreeMap;

use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-service";
pub const IPC_MAGIC: u32 = u32::from_le_bytes(*b"NXIP");
pub const IPC_VERSION: u32 = 1;
const COMMAND_HEADER_WORDS: usize = 13;
const RESPONSE_HEADER_WORDS: usize = 7;
const FLAG_HAS_DOMAIN: u64 = 1;
const FLAG_HAS_PROCESS_ID: u64 = 1 << 1;

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResultCode(pub u32);

impl ResultCode {
    pub const SUCCESS: Self = Self(0);
    pub const INVALID_COMMAND_BUFFER: Self = Self(0xE001);
    pub const INVALID_HANDLE: Self = Self(0xE002);
    pub const UNSUPPORTED_COMMAND: Self = Self(0xE003);

    #[must_use]
    pub const fn is_success(self) -> bool {
        self.0 == Self::SUCCESS.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionHandle(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DomainId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectHandle(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GuestHandle(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestServiceName {
    FileSystem,
    Thread,
    Memory,
    Input,
    AudioOut,
    Unknown(u32),
}

impl GuestServiceName {
    pub const AUDIO_OUT_ID: u32 = 0x6175_646F;

    #[must_use]
    pub const fn from_id(id: u32) -> Self {
        match id {
            1 => Self::FileSystem,
            2 => Self::Thread,
            3 => Self::Memory,
            4 => Self::Input,
            Self::AUDIO_OUT_ID => Self::AudioOut,
            value => Self::Unknown(value),
        }
    }

    #[must_use]
    pub const fn id(self) -> u32 {
        match self {
            Self::FileSystem => 1,
            Self::Thread => 2,
            Self::Memory => 3,
            Self::Input => 4,
            Self::AudioOut => Self::AUDIO_OUT_ID,
            Self::Unknown(value) => value,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileSystem => "filesystem",
            Self::Thread => "thread",
            Self::Memory => "memory",
            Self::Input => "input",
            Self::AudioOut => "audout:u",
            Self::Unknown(_) => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandType {
    Request,
    Control,
    Close,
    Response,
    Unknown(u32),
}

impl CommandType {
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::Request,
            2 => Self::Control,
            3 => Self::Close,
            4 => Self::Response,
            value => Self::Unknown(value),
        }
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        match self {
            Self::Request => 1,
            Self::Control => 2,
            Self::Close => 3,
            Self::Response => 4,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferDescriptorKind {
    Static,
    Send,
    Receive,
    Exchange,
}

impl BufferDescriptorKind {
    fn from_raw(raw: u64) -> Result<Self, IpcParseError> {
        match raw {
            0 => Ok(Self::Static),
            1 => Ok(Self::Send),
            2 => Ok(Self::Receive),
            3 => Ok(Self::Exchange),
            value => Err(IpcParseError::UnknownDescriptorKind { value }),
        }
    }

    const fn raw(self) -> u64 {
        match self {
            Self::Static => 0,
            Self::Send => 1,
            Self::Receive => 2,
            Self::Exchange => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferDescriptor {
    pub kind: BufferDescriptorKind,
    pub index: u32,
    pub address: u64,
    pub size: u64,
}

impl BufferDescriptor {
    #[must_use]
    pub const fn new(kind: BufferDescriptorKind, index: u32, address: u64, size: u64) -> Self {
        Self {
            kind,
            index,
            address,
            size,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleMode {
    Copy,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandleTransfer {
    pub mode: HandleMode,
    pub handle: GuestHandle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestCommand {
    pub command_type: CommandType,
    pub command_id: u32,
    pub service: GuestServiceName,
    pub session: SessionHandle,
    pub domain: Option<DomainId>,
    pub process_id: Option<ProcessId>,
    pub payload: Vec<u64>,
    pub descriptors: Vec<BufferDescriptor>,
    pub copy_handles: Vec<GuestHandle>,
    pub move_handles: Vec<GuestHandle>,
    pub objects: Vec<ObjectHandle>,
}

impl GuestCommand {
    #[must_use]
    pub fn request(service: GuestServiceName, session: SessionHandle, command_id: u32) -> Self {
        Self {
            command_type: CommandType::Request,
            command_id,
            service,
            session,
            domain: None,
            process_id: None,
            payload: Vec::new(),
            descriptors: Vec::new(),
            copy_handles: Vec::new(),
            move_handles: Vec::new(),
            objects: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_domain(mut self, domain: DomainId) -> Self {
        self.domain = Some(domain);
        self
    }

    #[must_use]
    pub fn with_process_id(mut self, process_id: ProcessId) -> Self {
        self.process_id = Some(process_id);
        self
    }

    #[must_use]
    pub fn with_payload(mut self, payload: impl Into<Vec<u64>>) -> Self {
        self.payload = payload.into();
        self
    }

    #[must_use]
    pub fn with_descriptor(mut self, descriptor: BufferDescriptor) -> Self {
        self.descriptors.push(descriptor);
        self
    }

    #[must_use]
    pub fn with_copy_handle(mut self, handle: GuestHandle) -> Self {
        self.copy_handles.push(handle);
        self
    }

    #[must_use]
    pub fn with_move_handle(mut self, handle: GuestHandle) -> Self {
        self.move_handles.push(handle);
        self
    }

    #[must_use]
    pub fn with_object(mut self, object: ObjectHandle) -> Self {
        self.objects.push(object);
        self
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut words = vec![
            header_word(),
            u64::from(self.command_type.raw()),
            u64::from(self.command_id),
            u64::from(self.service.id()),
            u64::from(self.session.0),
            u64::from(self.domain.map_or(0, |domain| domain.0)),
            self.flags(),
            self.process_id.map_or(0, |process_id| process_id.0),
            self.payload.len() as u64,
            self.descriptors.len() as u64,
            self.copy_handles.len() as u64,
            self.move_handles.len() as u64,
            self.objects.len() as u64,
        ];
        words.extend_from_slice(&self.payload);
        for descriptor in &self.descriptors {
            words.extend_from_slice(&[
                descriptor.kind.raw(),
                u64::from(descriptor.index),
                descriptor.address,
                descriptor.size,
            ]);
        }
        words.extend(self.copy_handles.iter().map(|handle| u64::from(handle.0)));
        words.extend(self.move_handles.iter().map(|handle| u64::from(handle.0)));
        words.extend(self.objects.iter().map(|object| u64::from(object.0)));
        words_to_bytes(&words)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, IpcParseError> {
        let words = words_from_bytes(bytes)?;
        if words.len() < COMMAND_HEADER_WORDS {
            return Err(IpcParseError::TooShort {
                expected_words: COMMAND_HEADER_WORDS,
                actual_words: words.len(),
            });
        }
        validate_header(words[0])?;

        let payload_count = to_usize(words[8], "payload")?;
        let descriptor_count = to_usize(words[9], "descriptors")?;
        let copy_count = to_usize(words[10], "copy handles")?;
        let move_count = to_usize(words[11], "move handles")?;
        let object_count = to_usize(words[12], "objects")?;
        let descriptor_words = descriptor_count
            .checked_mul(4)
            .ok_or(IpcParseError::CountOverflow)?;
        let expected_words = COMMAND_HEADER_WORDS
            .checked_add(payload_count)
            .and_then(|value| value.checked_add(descriptor_words))
            .and_then(|value| value.checked_add(copy_count))
            .and_then(|value| value.checked_add(move_count))
            .and_then(|value| value.checked_add(object_count))
            .ok_or(IpcParseError::CountOverflow)?;
        if words.len() != expected_words {
            return Err(IpcParseError::LengthMismatch {
                expected_words,
                actual_words: words.len(),
            });
        }

        let flags = words[6];
        let mut cursor = COMMAND_HEADER_WORDS;
        let payload = words[cursor..cursor + payload_count].to_vec();
        cursor += payload_count;

        let mut descriptors = Vec::with_capacity(descriptor_count);
        for _ in 0..descriptor_count {
            let kind = BufferDescriptorKind::from_raw(words[cursor])?;
            let index = to_u32(words[cursor + 1], "descriptor index")?;
            descriptors.push(BufferDescriptor::new(
                kind,
                index,
                words[cursor + 2],
                words[cursor + 3],
            ));
            cursor += 4;
        }

        let copy_handles = words[cursor..cursor + copy_count]
            .iter()
            .map(|word| to_u32(*word, "copy handle").map(GuestHandle))
            .collect::<Result<Vec<_>, _>>()?;
        cursor += copy_count;
        let move_handles = words[cursor..cursor + move_count]
            .iter()
            .map(|word| to_u32(*word, "move handle").map(GuestHandle))
            .collect::<Result<Vec<_>, _>>()?;
        cursor += move_count;
        let objects = words[cursor..cursor + object_count]
            .iter()
            .map(|word| to_u32(*word, "object handle").map(ObjectHandle))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            command_type: CommandType::from_raw(to_u32(words[1], "command type")?),
            command_id: to_u32(words[2], "command id")?,
            service: GuestServiceName::from_id(to_u32(words[3], "service id")?),
            session: SessionHandle(to_u32(words[4], "session handle")?),
            domain: (flags & FLAG_HAS_DOMAIN != 0)
                .then_some(DomainId(to_u32(words[5], "domain id")?)),
            process_id: (flags & FLAG_HAS_PROCESS_ID != 0).then_some(ProcessId(words[7])),
            payload,
            descriptors,
            copy_handles,
            move_handles,
            objects,
        })
    }

    fn flags(&self) -> u64 {
        let mut flags = 0;
        if self.domain.is_some() {
            flags |= FLAG_HAS_DOMAIN;
        }
        if self.process_id.is_some() {
            flags |= FLAG_HAS_PROCESS_ID;
        }
        flags
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestResponse {
    pub result: ResultCode,
    pub payload: Vec<u64>,
    pub copy_handles: Vec<GuestHandle>,
    pub move_handles: Vec<GuestHandle>,
    pub objects: Vec<ObjectHandle>,
}

impl GuestResponse {
    #[must_use]
    pub fn success(payload: impl Into<Vec<u64>>) -> Self {
        Self {
            result: ResultCode::SUCCESS,
            payload: payload.into(),
            copy_handles: Vec::new(),
            move_handles: Vec::new(),
            objects: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_object(mut self, object: ObjectHandle) -> Self {
        self.objects.push(object);
        self
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut words = vec![
            header_word(),
            u64::from(CommandType::Response.raw()),
            u64::from(self.result.0),
            self.payload.len() as u64,
            self.copy_handles.len() as u64,
            self.move_handles.len() as u64,
            self.objects.len() as u64,
        ];
        words.extend_from_slice(&self.payload);
        words.extend(self.copy_handles.iter().map(|handle| u64::from(handle.0)));
        words.extend(self.move_handles.iter().map(|handle| u64::from(handle.0)));
        words.extend(self.objects.iter().map(|object| u64::from(object.0)));
        words_to_bytes(&words)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, IpcParseError> {
        let words = words_from_bytes(bytes)?;
        if words.len() < RESPONSE_HEADER_WORDS {
            return Err(IpcParseError::TooShort {
                expected_words: RESPONSE_HEADER_WORDS,
                actual_words: words.len(),
            });
        }
        validate_header(words[0])?;
        if CommandType::from_raw(to_u32(words[1], "response type")?) != CommandType::Response {
            return Err(IpcParseError::InvalidResponseType { value: words[1] });
        }

        let payload_count = to_usize(words[3], "response payload")?;
        let copy_count = to_usize(words[4], "response copy handles")?;
        let move_count = to_usize(words[5], "response move handles")?;
        let object_count = to_usize(words[6], "response objects")?;
        let expected_words = RESPONSE_HEADER_WORDS
            .checked_add(payload_count)
            .and_then(|value| value.checked_add(copy_count))
            .and_then(|value| value.checked_add(move_count))
            .and_then(|value| value.checked_add(object_count))
            .ok_or(IpcParseError::CountOverflow)?;
        if words.len() != expected_words {
            return Err(IpcParseError::LengthMismatch {
                expected_words,
                actual_words: words.len(),
            });
        }

        let mut cursor = RESPONSE_HEADER_WORDS;
        let payload = words[cursor..cursor + payload_count].to_vec();
        cursor += payload_count;
        let copy_handles = words[cursor..cursor + copy_count]
            .iter()
            .map(|word| to_u32(*word, "response copy handle").map(GuestHandle))
            .collect::<Result<Vec<_>, _>>()?;
        cursor += copy_count;
        let move_handles = words[cursor..cursor + move_count]
            .iter()
            .map(|word| to_u32(*word, "response move handle").map(GuestHandle))
            .collect::<Result<Vec<_>, _>>()?;
        cursor += move_count;
        let objects = words[cursor..cursor + object_count]
            .iter()
            .map(|word| to_u32(*word, "response object handle").map(ObjectHandle))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            result: ResultCode(to_u32(words[2], "response result")?),
            payload,
            copy_handles,
            move_handles,
            objects,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedCommand {
    pub service: GuestServiceName,
    pub session: SessionHandle,
    pub domain: Option<DomainId>,
    pub command_id: u32,
    pub command_type: CommandType,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionTable {
    next_session: u32,
    next_domain: u32,
    next_object: u32,
    sessions: BTreeMap<SessionHandle, GuestServiceName>,
    domains: BTreeMap<DomainId, SessionHandle>,
    objects: BTreeMap<ObjectHandle, ObjectRecord>,
}

impl SessionTable {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_session: 0x100,
            next_domain: 1,
            next_object: 1,
            sessions: BTreeMap::new(),
            domains: BTreeMap::new(),
            objects: BTreeMap::new(),
        }
    }

    pub fn open_service(&mut self, service: GuestServiceName) -> SessionHandle {
        let handle = SessionHandle(self.next_session);
        self.next_session = self.next_session.saturating_add(1);
        self.sessions.insert(handle, service);
        handle
    }

    pub fn close_session(&mut self, session: SessionHandle) -> Result<(), SessionError> {
        self.sessions
            .remove(&session)
            .ok_or(SessionError::UnknownSession { session })?;
        self.domains.retain(|_, owner| *owner != session);
        self.objects.retain(|_, object| object.session != session);
        Ok(())
    }

    pub fn open_domain(&mut self, session: SessionHandle) -> Result<DomainId, SessionError> {
        self.service_for_session(session)?;
        let domain = DomainId(self.next_domain);
        self.next_domain = self.next_domain.saturating_add(1);
        self.domains.insert(domain, session);
        Ok(domain)
    }

    pub fn add_object(
        &mut self,
        domain: DomainId,
        service: GuestServiceName,
    ) -> Result<ObjectHandle, SessionError> {
        let session = *self
            .domains
            .get(&domain)
            .ok_or(SessionError::UnknownDomain { domain })?;
        let object = ObjectHandle(self.next_object);
        self.next_object = self.next_object.saturating_add(1);
        self.objects.insert(
            object,
            ObjectRecord {
                domain,
                session,
                service,
            },
        );
        Ok(object)
    }

    pub fn service_for_session(
        &self,
        session: SessionHandle,
    ) -> Result<GuestServiceName, SessionError> {
        self.sessions
            .get(&session)
            .copied()
            .ok_or(SessionError::UnknownSession { session })
    }

    pub fn route(&self, command: &GuestCommand) -> Result<RoutedCommand, SessionError> {
        let service = self.service_for_session(command.session)?;
        if service != command.service {
            return Err(SessionError::ServiceMismatch {
                session: command.session,
                expected: service,
                actual: command.service,
            });
        }
        if let Some(domain) = command.domain {
            let owner = self
                .domains
                .get(&domain)
                .ok_or(SessionError::UnknownDomain { domain })?;
            if *owner != command.session {
                return Err(SessionError::DomainSessionMismatch {
                    domain,
                    expected: *owner,
                    actual: command.session,
                });
            }
        }
        for object in &command.objects {
            let record = self
                .objects
                .get(object)
                .ok_or(SessionError::UnknownObject { object: *object })?;
            if record.session != command.session {
                return Err(SessionError::ObjectSessionMismatch {
                    object: *object,
                    expected: record.session,
                    actual: command.session,
                });
            }
        }
        Ok(RoutedCommand {
            service,
            session: command.session,
            domain: command.domain,
            command_id: command.command_id,
            command_type: command.command_type,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ObjectRecord {
    domain: DomainId,
    session: SessionHandle,
    service: GuestServiceName,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IpcParseError {
    #[error("IPC command buffer length {len} is not 8-byte aligned")]
    Unaligned { len: usize },
    #[error("IPC command buffer has {actual_words} words, expected at least {expected_words}")]
    TooShort {
        expected_words: usize,
        actual_words: usize,
    },
    #[error("IPC command buffer magic {actual:#x} did not match expected {expected:#x}")]
    BadMagic { expected: u32, actual: u32 },
    #[error("IPC command buffer version {actual} did not match expected {expected}")]
    BadVersion { expected: u32, actual: u32 },
    #[error("IPC command buffer has {actual_words} words, expected {expected_words}")]
    LengthMismatch {
        expected_words: usize,
        actual_words: usize,
    },
    #[error("IPC command buffer count overflow")]
    CountOverflow,
    #[error("IPC field `{field}` value {value} does not fit in u32")]
    FieldTooLarge { field: &'static str, value: u64 },
    #[error("unknown IPC buffer descriptor kind {value}")]
    UnknownDescriptorKind { value: u64 },
    #[error("invalid IPC response type {value}")]
    InvalidResponseType { value: u64 },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("unknown guest IPC session {session:?}")]
    UnknownSession { session: SessionHandle },
    #[error("unknown guest IPC domain {domain:?}")]
    UnknownDomain { domain: DomainId },
    #[error("unknown guest IPC object {object:?}")]
    UnknownObject { object: ObjectHandle },
    #[error("session {session:?} is for {expected:?}, command targeted {actual:?}")]
    ServiceMismatch {
        session: SessionHandle,
        expected: GuestServiceName,
        actual: GuestServiceName,
    },
    #[error("domain {domain:?} belongs to {expected:?}, command used {actual:?}")]
    DomainSessionMismatch {
        domain: DomainId,
        expected: SessionHandle,
        actual: SessionHandle,
    },
    #[error("object {object:?} belongs to {expected:?}, command used {actual:?}")]
    ObjectSessionMismatch {
        object: ObjectHandle,
        expected: SessionHandle,
        actual: SessionHandle,
    },
}

fn header_word() -> u64 {
    u64::from(IPC_MAGIC) | (u64::from(IPC_VERSION) << 32)
}

fn validate_header(word: u64) -> Result<(), IpcParseError> {
    let actual_magic = word as u32;
    let actual_version = (word >> 32) as u32;
    if actual_magic != IPC_MAGIC {
        return Err(IpcParseError::BadMagic {
            expected: IPC_MAGIC,
            actual: actual_magic,
        });
    }
    if actual_version != IPC_VERSION {
        return Err(IpcParseError::BadVersion {
            expected: IPC_VERSION,
            actual: actual_version,
        });
    }
    Ok(())
}

fn words_from_bytes(bytes: &[u8]) -> Result<Vec<u64>, IpcParseError> {
    if !bytes.len().is_multiple_of(8) {
        return Err(IpcParseError::Unaligned { len: bytes.len() });
    }
    let mut words = Vec::with_capacity(bytes.len() / 8);
    for chunk in bytes.chunks_exact(8) {
        let mut word = [0; 8];
        word.copy_from_slice(chunk);
        words.push(u64::from_le_bytes(word));
    }
    Ok(words)
}

fn words_to_bytes(words: &[u64]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(words.len() * 8);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes
}

fn to_u32(value: u64, field: &'static str) -> Result<u32, IpcParseError> {
    u32::try_from(value).map_err(|_| IpcParseError::FieldTooLarge { field, value })
}

fn to_usize(value: u64, field: &'static str) -> Result<usize, IpcParseError> {
    let value = to_u32(value, field)?;
    usize::try_from(value).map_err(|_| IpcParseError::FieldTooLarge {
        field,
        value: u64::from(value),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        BufferDescriptor, BufferDescriptorKind, CommandType, DomainId, GuestCommand, GuestHandle,
        GuestResponse, GuestServiceName, IpcParseError, ObjectHandle, ProcessId, ResultCode,
        SessionError, SessionHandle, SessionTable,
    };

    #[test]
    fn command_buffer_round_trips_all_descriptor_and_handle_classes() {
        let command = GuestCommand::request(GuestServiceName::AudioOut, SessionHandle(0x100), 0x42)
            .with_domain(DomainId(7))
            .with_process_id(ProcessId(99))
            .with_payload([48_000, 120])
            .with_descriptor(BufferDescriptor::new(
                BufferDescriptorKind::Static,
                0,
                0x1000,
                16,
            ))
            .with_descriptor(BufferDescriptor::new(
                BufferDescriptorKind::Send,
                1,
                0x2000,
                32,
            ))
            .with_descriptor(BufferDescriptor::new(
                BufferDescriptorKind::Receive,
                2,
                0x3000,
                64,
            ))
            .with_descriptor(BufferDescriptor::new(
                BufferDescriptorKind::Exchange,
                3,
                0x4000,
                128,
            ))
            .with_copy_handle(GuestHandle(10))
            .with_move_handle(GuestHandle(11))
            .with_object(ObjectHandle(12));

        let decoded = GuestCommand::from_bytes(&command.to_bytes()).expect("command should parse");

        assert_eq!(decoded, command);
    }

    #[test]
    fn command_parser_rejects_invalid_header_and_lengths() {
        assert_eq!(
            GuestCommand::from_bytes(&[1, 2, 3]).expect_err("unaligned input should fail"),
            IpcParseError::Unaligned { len: 3 }
        );

        let mut bytes =
            GuestCommand::request(GuestServiceName::AudioOut, SessionHandle(1), 1).to_bytes();
        bytes[0] = 0;
        assert!(matches!(
            GuestCommand::from_bytes(&bytes),
            Err(IpcParseError::BadMagic { .. })
        ));

        let truncated = GuestCommand::request(GuestServiceName::AudioOut, SessionHandle(1), 1)
            .with_payload([1, 2])
            .to_bytes();
        assert!(matches!(
            GuestCommand::from_bytes(&truncated[..truncated.len() - 8]),
            Err(IpcParseError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn response_buffer_round_trips_result_payload_and_objects() {
        let mut response = GuestResponse::success([4, 5]).with_object(ObjectHandle(9));
        response.result = ResultCode::SUCCESS;
        response.copy_handles.push(GuestHandle(1));
        response.move_handles.push(GuestHandle(2));

        let decoded = GuestResponse::from_bytes(&response.to_bytes()).expect("response parses");

        assert_eq!(decoded, response);
        assert!(decoded.result.is_success());
    }

    #[test]
    fn session_table_routes_sessions_domains_and_objects() {
        let mut sessions = SessionTable::new();
        let session = sessions.open_service(GuestServiceName::AudioOut);
        let domain = sessions.open_domain(session).expect("domain opens");
        let object = sessions
            .add_object(domain, GuestServiceName::AudioOut)
            .expect("object opens");
        let command = GuestCommand::request(GuestServiceName::AudioOut, session, 1)
            .with_domain(domain)
            .with_object(object);

        let routed = sessions.route(&command).expect("command should route");

        assert_eq!(routed.service, GuestServiceName::AudioOut);
        assert_eq!(routed.command_type, CommandType::Request);
        assert_eq!(routed.command_id, 1);
        assert_eq!(routed.domain, Some(domain));
    }

    #[test]
    fn session_table_rejects_mismatched_service() {
        let mut sessions = SessionTable::new();
        let session = sessions.open_service(GuestServiceName::AudioOut);
        let command = GuestCommand::request(GuestServiceName::FileSystem, session, 1);

        assert!(matches!(
            sessions.route(&command),
            Err(SessionError::ServiceMismatch { .. })
        ));
    }
}
