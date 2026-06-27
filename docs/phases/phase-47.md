# Phase 47: Guest IPC v0 and Audio Runtime Skeleton

Phase 47 expands the audio skeleton into the first guest service IPC foundation.
Homebrew can open an audio service session, submit a binary guest IPC command
buffer from mapped guest memory, and queue interleaved stereo `f32` PCM into the
audio runtime.

## What it does

- **Guest IPC v0** - `nx86-service` defines service names, sessions, domains,
  object handles, result codes, handle transfers, buffer descriptors, and binary
  command/response buffer round trips.
- **Audio output abstraction** - `nx86-audio` owns `AudioRuntime`, a `cpal`
  host output backend, and a null sink fallback for headless machines.
- **Buffer model** - audio buffers are interleaved stereo `f32` samples with a
  non-zero sample rate. The runtime tracks submitted, queued, consumed, and
  underflowed frames.
- **Audio service path** - homebrew uses `svc #5` to open `audout:u` and
  `svc #6` to dispatch a guest IPC command buffer. Runtime dispatch parses the
  command buffer, routes it through the guest session table, reads the described
  guest PCM buffer, and queues it to audio.
- **Timing tests** - the null sink exposes an injected deterministic frame clock
  for queue-consumption and underflow tests. Host `cpal` playback consumes
  through its audio callback.
- **Settings status** - the GUI Settings screen shows audio backend status,
  sample rate, channel count, queued frames, underflows, and a mute toggle.

## Design

Guest IPC lives in `nx86-service`, not `nx86-core::ipc`. The existing
`nx86-core::ipc` module remains host worker JSON-line IPC between the GUI and
worker process. This keeps guest service command buffers separate from host
application control messages.

The Phase 47 wire format is a clean-room Nx86 guest IPC v0 format with
Horizon-like concepts: sessions, domains, command IDs, result codes, copied and
moved handles, process IDs, object returns, and static/send/receive/exchange
buffer descriptors. It is binary and deterministic, but it is not claimed to be
complete Horizon IPC compatibility.

The HLE crate classifies `audout:u` service events. The runtime owns guest
memory reads, session-table routing, and audio buffer dispatch because only the
runtime has the guest `GuestMemory` and the audio backend.

## Phase boundary

Phase 47 does not implement real Horizon audio mixing, device selection,
resampling, surround layouts, command-buffer compatibility beyond the Nx86 v0
subset, or commercial-title service coverage. Those remain later HLE/runtime
compatibility work.
