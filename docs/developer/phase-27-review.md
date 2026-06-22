# Phase 27 Review

Date: 2026-06-21

## Scope

This review covers Phase 27 from `SPEC.md`: an Inspector v0 with a disassembly
view, function list, block list, NxIR view, and native mapping view. The exit
criterion is that a user can inspect a synthetic program. Per the chosen
direction, the Inspector sources its program through the title database (the
Library flow) rather than from synthetic files directly.

## Findings

- A new `nx86-inspector` crate (`crates/nx86-inspector/src/lib.rs`) provides
  `inspect_program(&[u8], entry) -> Result<InspectorReport, InspectError>`. It
  composes existing APIs only — `decode_program`, `recover_cfg` (Phase 26),
  `lift_program` → `Function::dump`, and `lower_function` — and adds no new
  analysis. It is host-independent and carries no `unsafe`, mmap, or transmute.
- Decoding and CFG recovery are prerequisites (`InspectError`); the NxIR
  (`NxirView`) and native (`NativeView`) views degrade to an `Unavailable(reason)`
  string when lifting or lowering fails, so a program that cannot compile still
  yields disassembly and a recovered CFG. This matches SPEC §11.4 ("the Inspector
  MAY inspect a title even if the title cannot compile").
- Native lowering is pure byte emission, so the native-mapping view is populated
  on the Apple Silicon dev host; only execution of those bytes is Linux-only and
  is explicitly out of v0 scope.
- `nx86-title-db` gained `TitleSourceKind::Synthetic`,
  `create_synthetic_title(...)`, and `read_content(...)`. The synthetic program
  TOML is persisted under the title's `content/` directory and recorded in
  `content_path`; the database only stores/reads the caller-supplied string, so
  it stays decoupled from `nx86-testsuite`. The shared `INSERT` was factored into
  `insert_title` so placeholder and synthetic creation share one row writer.
- `nx86-gui` adds `AppScreen::Inspector`, a Library "Import Synthetic Test…"
  action, and an Inspector screen that lists titles, inspects any title that
  carries content (placeholders show "no content"), and renders the title
  structure plus the five views with the existing monospace scroll-area dump
  pattern. The GUI layer builds the rendered strings so the screen module only
  renders text.

## Boundary Checks

- The only program bytes a title can acquire are the project's own synthetic
  test format, validated by `SyntheticArm64Test::parse` before storage. No game
  dumps, firmware, console keys, SDK code, or copyrighted blobs are imported or
  embedded (`CLAUDE.md` hard rule).
- Inspection is pure analysis: no native code is executed, no new `unsafe`, and
  no new mmap/transmute boundaries. The native view shows lowered bytes and
  their disassembly, never a call into generated code.
- No `unwrap`/`todo`/`dbg` in the new code; inspection reports `InspectError` or
  an `Unavailable` reason rather than panicking on malformed input.
- The GUI remains Linux-only to run; the new logic is exercised by
  host-independent unit tests on the dev host.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets   # 175 tests, 0 failures
```

New tests: four in `nx86-inspector` (a straight-line program; a forward branch
that prunes its dead fall-through; a deterministic combined report; a misaligned
length reported as an error), two in `nx86-title-db` (synthetic content
round-trips through the database and sidecar; placeholders carry no content), and
one in `nx86-gui` (the Inspector view is built from synthetic title content,
covering the report → rendered-strings wiring). All are host-independent.
