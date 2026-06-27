use std::collections::VecDeque;
#[cfg(feature = "host-cpal")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "host-cpal")]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-audio";
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;
pub const STEREO_CHANNELS: u16 = 2;

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioBackendKind {
    Cpal,
    NullSink,
}

impl AudioBackendKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cpal => "cpal",
            Self::NullSink => "null sink",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioStatus {
    pub backend: AudioBackendKind,
    pub available: bool,
    pub device_name: Option<String>,
    pub fallback_reason: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub queued_frames: u64,
    pub submitted_frames: u64,
    pub consumed_frames: u64,
    pub underflows: u64,
    pub muted: bool,
}

impl AudioStatus {
    #[must_use]
    pub fn label(&self) -> String {
        if self.available {
            match &self.device_name {
                Some(device) => format!("{} ({device})", self.backend.label()),
                None => self.backend.label().to_owned(),
            }
        } else {
            self.fallback_reason
                .clone()
                .unwrap_or_else(|| "audio backend unavailable".to_owned())
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl AudioBuffer {
    pub fn stereo_f32(samples: Vec<f32>, sample_rate: u32) -> Result<Self, AudioError> {
        if sample_rate == 0 {
            return Err(AudioError::InvalidSampleRate { sample_rate });
        }
        if !samples.len().is_multiple_of(usize::from(STEREO_CHANNELS)) {
            return Err(AudioError::UnalignedStereoSamples {
                samples: samples.len(),
            });
        }
        Ok(Self {
            samples,
            sample_rate,
        })
    }

    #[must_use]
    pub fn frame_count(&self) -> u64 {
        (self.samples.len() / usize::from(STEREO_CHANNELS)) as u64
    }
}

pub struct AudioRuntime {
    backend: AudioBackend,
}

impl AudioRuntime {
    #[must_use]
    pub fn new() -> Self {
        #[cfg(feature = "host-cpal")]
        {
            Self {
                backend: CpalAudioBackend::try_new()
                    .map(AudioBackend::Cpal)
                    .unwrap_or_else(|error| {
                        AudioBackend::Null(NullAudioSink::new(Some(error.to_string())))
                    }),
            }
        }

        #[cfg(not(feature = "host-cpal"))]
        {
            Self::null("host audio backend disabled at build time")
        }
    }

    #[must_use]
    pub fn null(reason: impl Into<String>) -> Self {
        Self {
            backend: AudioBackend::Null(NullAudioSink::new(Some(reason.into()))),
        }
    }

    pub fn enqueue(&mut self, buffer: AudioBuffer) -> Result<u64, AudioError> {
        self.backend.enqueue(buffer)
    }

    pub fn advance_test_clock(&mut self, frames: u64) {
        self.backend.advance_test_clock(frames);
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.backend.set_muted(muted);
    }

    #[must_use]
    pub fn status(&self) -> AudioStatus {
        self.backend.status()
    }
}

impl Default for AudioRuntime {
    fn default() -> Self {
        Self::new()
    }
}

enum AudioBackend {
    #[cfg(feature = "host-cpal")]
    Cpal(CpalAudioBackend),
    Null(NullAudioSink),
}

impl AudioBackend {
    fn enqueue(&mut self, buffer: AudioBuffer) -> Result<u64, AudioError> {
        match self {
            #[cfg(feature = "host-cpal")]
            Self::Cpal(backend) => backend.enqueue(buffer),
            Self::Null(backend) => Ok(backend.enqueue(buffer)),
        }
    }

    fn advance_test_clock(&mut self, frames: u64) {
        match self {
            #[cfg(feature = "host-cpal")]
            Self::Cpal(_) => {}
            Self::Null(backend) => backend.advance(frames),
        }
    }

    fn set_muted(&mut self, muted: bool) {
        match self {
            #[cfg(feature = "host-cpal")]
            Self::Cpal(backend) => backend.set_muted(muted),
            Self::Null(backend) => backend.set_muted(muted),
        }
    }

    fn status(&self) -> AudioStatus {
        match self {
            #[cfg(feature = "host-cpal")]
            Self::Cpal(backend) => backend.status(),
            Self::Null(backend) => backend.status(),
        }
    }
}

#[cfg(feature = "host-cpal")]
struct CpalAudioBackend {
    shared: Arc<Mutex<AudioShared>>,
    _stream: cpal::Stream,
    device_name: Option<String>,
}

#[cfg(feature = "host-cpal")]
impl CpalAudioBackend {
    fn try_new() -> Result<Self, AudioInitError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioInitError::NoOutputDevice)?;
        let supported = device
            .default_output_config()
            .map_err(AudioInitError::DefaultConfig)?;
        let sample_rate = supported.sample_rate();
        let channels = supported.channels();
        let shared = Arc::new(Mutex::new(AudioShared::new(sample_rate, channels, false)));
        let config: cpal::StreamConfig = supported.into();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build_stream_f32(&device, &config, Arc::clone(&shared)),
            cpal::SampleFormat::I16 => build_stream_i16(&device, &config, Arc::clone(&shared)),
            cpal::SampleFormat::U16 => build_stream_u16(&device, &config, Arc::clone(&shared)),
            format => Err(AudioInitError::UnsupportedSampleFormat {
                format: format!("{format:?}"),
            }),
        }?;
        stream.play().map_err(AudioInitError::PlayStream)?;
        Ok(Self {
            shared,
            _stream: stream,
            device_name: None,
        })
    }

    fn enqueue(&mut self, buffer: AudioBuffer) -> Result<u64, AudioError> {
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| AudioError::BackendUnavailable("audio state lock poisoned".to_owned()))?;
        Ok(shared.enqueue(buffer.samples, buffer.sample_rate))
    }

    fn set_muted(&mut self, muted: bool) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.muted = muted;
        }
    }

    fn status(&self) -> AudioStatus {
        self.shared.lock().map_or_else(
            |_| AudioStatus {
                backend: AudioBackendKind::Cpal,
                available: false,
                device_name: self.device_name.clone(),
                fallback_reason: Some("audio state lock poisoned".to_owned()),
                sample_rate: DEFAULT_SAMPLE_RATE,
                channels: STEREO_CHANNELS,
                queued_frames: 0,
                submitted_frames: 0,
                consumed_frames: 0,
                underflows: 0,
                muted: false,
            },
            |shared| shared.status(AudioBackendKind::Cpal, true, self.device_name.clone(), None),
        )
    }
}

#[derive(Clone, Debug)]
struct NullAudioSink {
    shared: AudioShared,
    reason: Option<String>,
}

impl NullAudioSink {
    fn new(reason: Option<String>) -> Self {
        Self {
            shared: AudioShared::new(DEFAULT_SAMPLE_RATE, STEREO_CHANNELS, false),
            reason,
        }
    }

    fn enqueue(&mut self, buffer: AudioBuffer) -> u64 {
        self.shared.enqueue(buffer.samples, buffer.sample_rate)
    }

    fn advance(&mut self, frames: u64) {
        self.shared.consume_frames(frames);
    }

    fn set_muted(&mut self, muted: bool) {
        self.shared.muted = muted;
    }

    fn status(&self) -> AudioStatus {
        self.shared
            .status(AudioBackendKind::NullSink, false, None, self.reason.clone())
    }
}

#[derive(Clone, Debug)]
struct AudioShared {
    samples: VecDeque<f32>,
    sample_rate: u32,
    output_channels: u16,
    submitted_frames: u64,
    consumed_frames: u64,
    underflows: u64,
    muted: bool,
}

impl AudioShared {
    fn new(sample_rate: u32, channels: u16, muted: bool) -> Self {
        Self {
            samples: VecDeque::new(),
            sample_rate,
            output_channels: channels.max(1),
            submitted_frames: 0,
            consumed_frames: 0,
            underflows: 0,
            muted,
        }
    }

    fn enqueue(&mut self, samples: Vec<f32>, sample_rate: u32) -> u64 {
        self.sample_rate = sample_rate;
        let frames = samples.len() as u64 / u64::from(STEREO_CHANNELS);
        self.submitted_frames = self.submitted_frames.saturating_add(frames);
        self.samples.extend(samples);
        self.queued_frames()
    }

    fn consume_frames(&mut self, frames: u64) {
        let available_frames = self.queued_frames();
        let consumed_frames = frames.min(available_frames);
        for _ in 0..consumed_frames.saturating_mul(u64::from(STEREO_CHANNELS)) {
            let _ = self.samples.pop_front();
        }
        self.consumed_frames = self.consumed_frames.saturating_add(consumed_frames);
        if frames > consumed_frames {
            self.underflows = self.underflows.saturating_add(frames - consumed_frames);
        }
    }

    #[cfg(feature = "host-cpal")]
    fn fill_f32(&mut self, output: &mut [f32]) {
        let output_channels = usize::from(self.output_channels);
        for frame in output.chunks_mut(output_channels) {
            let Some(left) = self.samples.pop_front() else {
                frame.fill(0.0);
                self.underflows = self.underflows.saturating_add(1);
                continue;
            };
            let Some(right) = self.samples.pop_front() else {
                frame.fill(0.0);
                self.underflows = self.underflows.saturating_add(1);
                continue;
            };

            self.consumed_frames = self.consumed_frames.saturating_add(1);
            if self.muted {
                frame.fill(0.0);
                continue;
            }
            match frame {
                [] => {}
                [mono] => {
                    *mono = (left + right) * 0.5;
                }
                [first, second, rest @ ..] => {
                    *first = left;
                    *second = right;
                    rest.fill(0.0);
                }
            }
        }
    }

    fn queued_frames(&self) -> u64 {
        self.samples.len() as u64 / u64::from(STEREO_CHANNELS)
    }

    fn status(
        &self,
        backend: AudioBackendKind,
        available: bool,
        device_name: Option<String>,
        fallback_reason: Option<String>,
    ) -> AudioStatus {
        AudioStatus {
            backend,
            available,
            device_name,
            fallback_reason,
            sample_rate: self.sample_rate,
            channels: self.output_channels,
            queued_frames: self.queued_frames(),
            submitted_frames: self.submitted_frames,
            consumed_frames: self.consumed_frames,
            underflows: self.underflows,
            muted: self.muted,
        }
    }
}

#[cfg(feature = "host-cpal")]
fn build_stream_f32(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Mutex<AudioShared>>,
) -> Result<cpal::Stream, AudioInitError> {
    device
        .build_output_stream(
            *config,
            move |data: &mut [f32], _| fill_output_f32(data, &shared),
            |_error| {},
            None,
        )
        .map_err(AudioInitError::BuildStream)
}

#[cfg(feature = "host-cpal")]
fn build_stream_i16(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Mutex<AudioShared>>,
) -> Result<cpal::Stream, AudioInitError> {
    device
        .build_output_stream(
            *config,
            move |data: &mut [i16], _| {
                let mut scratch = vec![0.0; data.len()];
                fill_output_f32(&mut scratch, &shared);
                for (sample, value) in data.iter_mut().zip(scratch) {
                    *sample = (value.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                }
            },
            |_error| {},
            None,
        )
        .map_err(AudioInitError::BuildStream)
}

#[cfg(feature = "host-cpal")]
fn build_stream_u16(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Mutex<AudioShared>>,
) -> Result<cpal::Stream, AudioInitError> {
    device
        .build_output_stream(
            *config,
            move |data: &mut [u16], _| {
                let mut scratch = vec![0.0; data.len()];
                fill_output_f32(&mut scratch, &shared);
                for (sample, value) in data.iter_mut().zip(scratch) {
                    let normalized = value.clamp(-1.0, 1.0).mul_add(0.5, 0.5);
                    *sample = (normalized * f32::from(u16::MAX)) as u16;
                }
            },
            |_error| {},
            None,
        )
        .map_err(AudioInitError::BuildStream)
}

#[cfg(feature = "host-cpal")]
fn fill_output_f32(output: &mut [f32], shared: &Arc<Mutex<AudioShared>>) {
    if let Ok(mut shared) = shared.lock() {
        shared.fill_f32(output);
    } else {
        output.fill(0.0);
    }
}

#[derive(Debug, Error)]
#[cfg(feature = "host-cpal")]
pub enum AudioInitError {
    #[error("no host audio output device is available")]
    NoOutputDevice,
    #[error("failed to read host audio default output config: {0}")]
    DefaultConfig(cpal::Error),
    #[error("unsupported host audio sample format {format}")]
    UnsupportedSampleFormat { format: String },
    #[error("failed to build host audio output stream: {0}")]
    BuildStream(cpal::Error),
    #[error("failed to start host audio output stream: {0}")]
    PlayStream(cpal::Error),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AudioError {
    #[error("audio buffer sample rate must be non-zero, got {sample_rate}")]
    InvalidSampleRate { sample_rate: u32 },
    #[error("stereo f32 audio buffer has {samples} samples, expected an even sample count")]
    UnalignedStereoSamples { samples: usize },
    #[error("audio backend is unavailable: {0}")]
    BackendUnavailable(String),
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "host-cpal")]
    use super::AudioShared;
    use super::{AudioBackendKind, AudioBuffer, AudioError, AudioRuntime};

    #[test]
    fn stereo_buffer_rejects_unaligned_sample_count() {
        assert_eq!(
            AudioBuffer::stereo_f32(vec![0.0, 1.0, 0.5], 48_000)
                .expect_err("odd sample count should fail"),
            AudioError::UnalignedStereoSamples { samples: 3 }
        );
    }

    #[test]
    fn null_sink_tracks_queue_consumption_and_underflows() {
        let mut runtime = AudioRuntime::null("test");
        let buffer = AudioBuffer::stereo_f32(vec![0.25, -0.25, 0.5, -0.5], 48_000)
            .expect("buffer should be valid");

        assert_eq!(runtime.enqueue(buffer).expect("enqueue succeeds"), 2);
        runtime.advance_test_clock(1);
        let status = runtime.status();
        assert_eq!(status.backend, AudioBackendKind::NullSink);
        assert_eq!(status.queued_frames, 1);
        assert_eq!(status.consumed_frames, 1);
        assert_eq!(status.underflows, 0);

        runtime.advance_test_clock(3);
        let status = runtime.status();
        assert_eq!(status.queued_frames, 0);
        assert_eq!(status.consumed_frames, 2);
        assert_eq!(status.underflows, 2);
    }

    #[test]
    fn null_sink_status_reports_fallback_reason_and_mute_state() {
        let mut runtime = AudioRuntime::null("headless");
        runtime.set_muted(true);

        let status = runtime.status();

        assert!(!status.available);
        assert_eq!(status.label(), "headless");
        assert!(status.muted);
    }

    #[test]
    #[cfg(feature = "host-cpal")]
    fn host_output_channel_count_does_not_change_stereo_queue_accounting() {
        let mut shared = AudioShared::new(48_000, 4, false);
        assert_eq!(shared.enqueue(vec![0.2, -0.2, 0.5, -0.5], 48_000), 2);

        let mut output = [1.0; 8];
        shared.fill_f32(&mut output);

        assert_eq!(output, [0.2, -0.2, 0.0, 0.0, 0.5, -0.5, 0.0, 0.0]);
        assert_eq!(shared.queued_frames(), 0);
        assert_eq!(shared.consumed_frames, 2);
        assert_eq!(shared.underflows, 0);
    }
}
