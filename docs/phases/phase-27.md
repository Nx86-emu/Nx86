# Phase 27: Inspector v0

Phase 27 turns the analysis built up through Phase 26 into something a user can
look at. The Inspector takes a title, derives the five views Phase 27 calls for —
disassembly, function list, block list, NxIR, and native mapping — and surfaces
them in the GUI. It builds directly on the recovered CFG from Phase 26 and the
existing decoder, lifter, and backend lowering; it adds no new analysis of its
own, only composition and presentation.

The Inspector sources its program **through the title database** (the Library
flow), not from synthetic files directly. Because titles were metadata-only
placeholders before this phase, Phase 27 adds a minimal, legally-clean bridge:
a title can be created from one of the project's own **synthetic** test programs,
which is persisted under the title and then inspected. Only the synthetic format
flows through here — never game dumps, firmware, or other copyrighted blobs.

## What it does

- **`nx86-inspector` crate** — `inspect_program(&[u8], entry) -> InspectorReport`
  composes the views from existing APIs:
  - disassembly from `nx86_arm64_decode::decode_program`,
  - the recovered function/block CFG from `nx86_arm64_lift::recover_cfg`
    (Phase 26),
  - the lifted NxIR dump from `lift_program(...)` → `Function::dump()`,
  - the native (x86_64) mapping from `nx86_x64_v4::lower_function`, rendered
    per block as `entry PC → bytes + x86_64 disassembly`.
  Decoding and CFG recovery are prerequisites; the NxIR and native views degrade
  gracefully to an "unavailable" reason when a program cannot be lifted or
  lowered, matching the rule that the Inspector MAY inspect a title even if it
  cannot compile. Native *lowering* is pure byte emission, so the native mapping
  view is populated on every host, including the Apple Silicon dev host; only
  *executing* those bytes remains Linux-only and is out of v0 scope.
- **Title content bridge (`nx86-title-db`)** — a new `TitleSourceKind::Synthetic`,
  `create_synthetic_title(title_id, display_name, program_toml)` (persists the
  program under `content/program.nxsynth.toml` and records `content_path`), and
  `read_content(&TitleEntry)`. The crate only stores/reads the caller-supplied
  string; synthetic parsing stays in the higher layers.
- **GUI Inspector screen (`nx86-gui`)** — a new `AppScreen::Inspector`. The
  Library screen gains an "Import Synthetic Test…" action; the Inspector screen
  lists titles, lets the user inspect any title that carries content, and renders
  the title structure plus the disassembly, function list, control-flow graph,
  NxIR, and native-mapping views using the existing monospace/scroll-area dump
  pattern.

## New types

- `InspectorReport` — `entry`, `disassembly: Vec<DecodedInstruction>`,
  `cfg: RecoveredCfg`, `nxir: NxirView`, `native: NativeView`, plus
  `disassembly_text`/`function_list_text`/`nxir_text`/`native_text`/`render_text`
  renderers.
- `NxirView` — `Dump(String)` or `Unavailable(String)`.
- `NativeView` — `Mapped(Vec<NativeBlockMapping>)` or `Unavailable(String)`.
- `NativeBlockMapping` — `entry_pc`, `byte_len`, `dump`.
- `InspectError` — `Decode`, `Recover`.
- `TitleSourceKind::Synthetic`; `InspectorUiState` / `InspectorView` /
  `InspectorAction` in the GUI.

## Exit Criteria

- A user can inspect a synthetic program: import it as a title, open the
  Inspector, and see its disassembly, recovered functions and blocks, NxIR, and
  native mapping.
- The recovered structure shown is the Phase 26 CFG; the NxIR and native views
  are best-effort and never panic on programs that cannot be lifted or lowered.
