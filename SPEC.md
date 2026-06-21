# Nx86 Technical Specification

## Version

**Spec version:** 0.4 draft  
**Primary spec file:** `SPEC.md`  
**Companion overview file:** `README.md`  
**Project name:** Nx86  
**Project type:** GUI-first Nintendo Switch emulator and AArch64-to-x86_64-v4 native binary translation system  
**Primary compilation technique:** Profile-Guided Feedback-Directed Binary Translation  
**Product name for compilation technique:** Continuous Dynamic Compilation  
**Primary implementation language:** Rust  
**Primary GUI framework:** heavily customized egui  
**Initial OS target:** Linux  
**Initial CPU target:** desktop x86_64-v4  
**Initial graphics backend:** Vulkan  
**Future graphics backend:** D3D12  
**Initial test target:** synthetic ARM64 programs  
**First real software target:** homebrew  
**v1 aspirational title target:** The Legend of Zelda: Tears of the Kingdom  
**Development model before v1.0:** closed source  
**Planned v1.0 license:** GPLv3  
**Main design goal:** make Switch software run as natively as possible on desktop x86_64-v4 hardware  
**Ultimate goal:** generate a portable Pure AOT native title folder that no longer requires the full Nx86 emulator application

---

## 1. Requirements Language

This specification uses requirement language inspired by RFC-style documents.

- **MUST** means the feature or behavior is required.
- **MUST NOT** means the feature or behavior is forbidden.
- **SHOULD** means the feature or behavior is strongly recommended.
- **SHOULD NOT** means the feature or behavior should be avoided.
- **MAY** means the feature or behavior is optional.
- **FUTURE** means it is intentionally not required in early versions but remains part of the long-term architecture.

Nx86 is intentionally ambitious. Some requirements describe the long-term destination rather than the first implementation milestone.

---

## 2. Project Summary

Nx86 is a GUI-first Nintendo Switch emulator designed around aggressive native recompilation.

Traditional emulators typically execute guest code through an interpreter or runtime JIT. Nx86 instead treats JIT as a fallback and discovery system. Nx86 attempts to analyze, lift, optimize, and compile as much guest AArch64 code as possible into native x86_64-v4 machine code before the user launches a title.

Runtime execution produces profile data. That data is fed back into future compilation passes. Over time, Nx86 should need less JIT, fewer runtime helpers, fewer slowmem paths, and fewer dynamic fallbacks.

This strategy is called:

> **Continuous Dynamic Compilation**

The formal technical name is:

> **Profile-Guided Feedback-Directed Binary Translation**

Nx86 is not designed for instant launch. Nx86 accepts longer first-time compilation and large caches to improve runtime smoothness, reduce translation overhead, and eventually enable Pure AOT output.

Core concept:

```text
Compile before play.
Run as native as possible.
Profile every fallback.
Promote runtime discoveries.
Rebuild better native code.
Move toward 100% Native Coverage.
```

---

## 3. One-Sentence Description

Nx86 is a GUI-first Nintendo Switch emulator built around Continuous Dynamic Compilation, a Profile-Guided Feedback-Directed Binary Translation system that aggressively recompiles AArch64 code, shaders, runtime-discovered code paths, and title-specific behavior into native x86_64-v4 output, with the long-term goal of producing a portable Pure AOT title folder that can run without the full Nx86 emulator application.

---

## 4. Design Philosophy

Nx86 follows these principles:

1. **AOT first.**
2. **JIT only when necessary.**
3. **Interpreter usage during gameplay is a bug.**
4. **Native Coverage should trend toward 100%.**
5. **Runtime feedback is compiler input.**
6. **Long compilation is acceptable.**
7. **Large caches are acceptable.**
8. **Desktop x86_64-v4 performance comes first.**
9. **Accuracy matters.**
10. **Performance wins when correctness is preserved.**
11. **Complexity is hidden by default.**
12. **Compiler internals are available to advanced users.**
13. **The GUI should be clean and casual.**
14. **The developer tools should be powerful.**
15. **The endgame is a native PC-style title folder.**

Nx86 is not a normal JIT emulator.

Nx86 is a native recompilation system wrapped in a clean emulator frontend.

---

## 5. Product Identity

### 5.1 Name

The official project name is:

```text
Nx86
```

Meaning:

```text
NX  = Nintendo Switch codename
x86 = primary native host architecture family
```

### 5.2 Branding Direction

Nx86 should feel:

1. clean
2. casual
3. polished
4. fast
5. technical but approachable
6. not corporate
7. not terminal-first
8. not visually overwhelming by default

The visual target is:

```text
clean emulator frontend
+
visual compiler dashboard
+
optional deep compiler/debugger tooling
```

### 5.3 UI Complexity Policy

Nx86 MUST hide complexity by default.

Casual users should see:

1. titles
2. compile status
3. launch button
4. Native Coverage
5. compatibility status
6. simple cache status
7. clear errors

Advanced users should be able to open:

1. logs
2. disassembly
3. CFG view
4. NxIR view
5. native x86 view
6. register allocation view
7. guard/deopt view
8. VMM view
9. scheduler view
10. shader/pipeline view
11. crash analysis

---

## 6. Major Goals

Nx86 MUST:

1. Be a real Nintendo Switch emulator.
2. Be GUI-first.
3. Use egui with heavy customization.
4. Target desktop Linux x86_64-v4 first.
5. Use a custom compiler backend from the start.
6. Use a shared IR for AOT and JIT.
7. Require initial compilation before launching a title.
8. Avoid run-while-compiling behavior.
9. Use JIT only as an emergency/discovery path.
10. Treat interpreter execution during gameplay as a bug.
11. Maintain an internal title database.
12. Organize titles by title ID.
13. Support file import, folder import, and library scanning.
14. Allow content to be referenced, copied, or moved.
15. Store AOT/cache/profile data inside Nx86-managed folders.
16. Support multiple storage locations.
17. Refuse unknown/invalid imports until v2.
18. Support updates and DLC.
19. Create separate caches for title/update versions when executable code changes.
20. Automatically invalidate affected caches when executable hashes change.
21. Use many small native code objects.
22. Support memory-mapped and RAM-loaded code objects.
23. Avoid cache compression by default.
24. Garbage collect old caches.
25. Perform shallow cache checks every launch.
26. Perform full verification after updates or crashes.
27. Reserve a 64 GB virtual address arena at startup.
28. Use a VMM from day one.
29. Support fastmem and slowmem.
30. Count slowmem as unoptimized/non-native for coverage.
31. Support memory mirroring in release.
32. Disable memory mirroring in debug.
33. Support executable page hashing.
34. Support self-modifying code detection.
35. Log executable writes with call stack and source block.
36. Support guest threading from day one.
37. Support optional fiber/task scheduling.
38. Support deterministic scheduling as a hidden developer option.
39. Save crash-window replay logs.
40. Analyze crash logs to improve rebuilds.
41. Support shader AOT in v1.
42. Use Vulkan first.
43. Add D3D12 later.
44. Support runtime-level FPS unlock.
45. Support resolution scaling globally and per title.
46. Support Guarded Speculative Optimization.
47. Support title behavior patches.
48. Allow required behavior patches to be enabled and undisableable.
49. Support profile sharing with user approval.
50. Require profile sanitization before export.
51. Support signed verified profiles.
52. Support centralized and decentralized profile distribution.
53. Use SQLite plus human-readable sidecars.
54. Provide a README and a separate SPEC.md.
55. Use strict requirement language in SPEC.md.
56. Include issue templates.
57. Include a detailed roadmap.
58. Eventually produce Pure AOT title folders.
59. Eventually support native PC-style executable output.
60. Keep the project architecture broad enough to target TOTK-class titles.

---

## 7. Non-Goals

Nx86 MUST NOT:

1. Be a JIT-first emulator.
2. Prioritize instant launch over native compilation.
3. Treat interpreter fallback as normal gameplay behavior.
4. Hide all compiler tooling from advanced users.
5. Compress native caches by default.
6. Run multiple games at once.
7. Automatically upload profile data.
8. Automatically download shared profiles without approval.
9. Show Pure AOT to normal users before it is real.
10. Present Pure AOT as available before 100% readiness.
11. Require a central private server controlled by the developer.
12. Ship copyrighted game assets.
13. Ship game dumps.
14. Ship proprietary firmware.
15. Ship console keys.
16. Ship proprietary SDK code.
17. Include copyrighted binary blobs in shared profiles.
18. Include saves in shared runtime profiles.
19. Include personal user data in shared profiles.
20. Allow invalid imports before v2.

---

## 8. Legal and Redistribution Boundary

Nx86 may operate on user-provided legally obtained content.

Nx86 MUST NOT ship:

1. copyrighted game dumps
2. proprietary firmware
3. console keys
4. copyrighted assets
5. proprietary SDK code
6. copied binary blobs in shared profiles
7. shared save files
8. extracted copyrighted runtime data

Nx86 MAY create a local Pure AOT title folder for the user's own local use.

If Nx86 copies user-provided game content/assets into a local Pure AOT folder, that folder MUST include a `legal.txt` warning.

Example `legal.txt` intent:

```text
This folder may contain user-owned game content and generated Nx86 native code.
Do not redistribute this folder.
Do not upload it.
Do not share it.
This folder is intended only for local use by the person who legally provided the original content.
```

`legal.txt` is a warning and documentation mechanism. It does not magically make redistribution legal.

---

## System Reimplementation Policy

Nx86 SHOULD reimplement every system, service, runtime, and firmware behavior that can reasonably be reimplemented.

Nx86 MUST NOT be designed as a firmware-first emulator.

The preferred compatibility strategy is:

```text
clean-room reimplementation
  ↓
high-level emulation
  ↓
native Nx86 runtime service
  ↓
stub only when safe
  ↓
user-provided local system file fallback only when unavoidable
```

Nx86 MUST NOT ship firmware, system archives, console keys, proprietary SDK code, or copyrighted system components.

Nx86 MUST NOT include instructions, links, workflows, or tooling for obtaining console keys, dumping firmware, bypassing encryption, or sharing system files.

Nx86 MAY support user-provided local system files when required for compatibility, but this MUST be treated as a compatibility fallback rather than the preferred architecture.

For synthetic ARM64 tests, homebrew, and Pure AOT research, Nx86 SHOULD avoid requiring firmware or keys entirely.

For commercial title compatibility, Nx86 SHOULD reimplement as much Horizon-like service behavior as possible through clean-room HLE and native Nx86 runtime libraries.

### Reimplementation Targets

Nx86 SHOULD prioritize reimplementing:

1. process management
2. thread management
3. synchronization primitives
4. virtual memory behavior
5. filesystem services
6. save data services
7. input services
8. gamepad services
9. audio services
10. graphics services
11. shader translation
12. GPU pipeline management
13. applet/service stubs
14. account/user service stubs
15. settings service stubs
16. time services
17. network service stubs
18. error/reporting services
19. logging services
20. runtime loader behavior

### Firmware Fallback Policy

Firmware or user-provided system files MAY be supported only when:

1. the behavior cannot yet be accurately reimplemented
2. the user provides the files locally
3. Nx86 does not redistribute them
4. Nx86 does not provide acquisition instructions
5. the fallback is clearly marked as compatibility-only
6. the long-term goal remains reimplementation

### Pure AOT Impact

Pure AOT output SHOULD depend on Nx86’s own native runtime libraries rather than proprietary firmware files.

A Pure AOT title folder SHOULD include Nx86-provided runtime libraries for:

1. input
2. audio
3. graphics
4. filesystem behavior
5. service behavior
6. windowing
7. gamepad support
8. title-specific compatibility behavior

Pure AOT SHOULD NOT require the full Nx86 emulator app.

The long-term goal is for Nx86 to replace firmware/runtime dependency with clean-room native runtime components wherever possible.

---

## 9. Target Platform Strategy

### 9.1 Initial Target

Nx86 initially targets:

```text
OS: Linux
CPU: x86_64-v4
Graphics: Vulkan thru ash
GUI: egui
```

### 9.2 Future Targets

Nx86 SHOULD eventually support:

1. x86_64-v3
2. generic x86_64
3. ARM64 hosts
4. Windows
5. D3D12
6. handheld PCs

### 9.3 x86_64-v4 First

The first backend SHOULD assume desktop-class x86_64-v4.

It MAY use:

1. wide vector instructions
2. many vector registers
3. mask registers
4. aggressive register allocation
5. large native code objects
6. profile-guided layout
7. AVX-512-style lowering
8. hot/cold splitting
9. direct block chaining
10. helper inlining in maximum optimization mode

The v4 backend SHOULD NOT compromise its design for handheld targets during early development.

### 9.4 Vulkan Binding Choice

Nx86 SHOULD use `ash` for the Vulkan backend.

`ash` is preferred because Nx86 requires low-level Vulkan control for shader translation, pipeline cache management, descriptor layout control, synchronization, command buffer recording, memory management, debug tooling, and future vendor-specific optimization.

Nx86 MUST wrap `ash` behind an internal `nx86-vulkan` abstraction. Raw Vulkan handles and unsafe Vulkan calls SHOULD NOT leak into higher-level emulator crates.

Higher-level graphics crates such as `wgpu` or `vulkano` are not preferred for the core renderer because Nx86 needs explicit Vulkan behavior rather than a portability abstraction.

---

## 10. User Experience Model

### 10.1 First Launch

Nx86 MUST include a first-launch wizard.

The wizard SHOULD configure:

1. game library folders
2. cache folder
3. profile folder
4. import behavior
5. copy/move/reference behavior
6. CPU target
7. compile thread cap
8. all-core warning
9. cache size
10. profile sharing
11. profile upload approval
12. profile download approval
13. graphics backend
14. shader AOT behavior
15. controller/input behavior
16. developer mode visibility

### 10.2 Default Title Flow

```text
Add title
  ↓
Scan title
  ↓
Create title folder
  ↓
Compile required native cache
  ↓
Launch title
  ↓
Profile missing paths
  ↓
Rebuild cache later
  ↓
Improve Native Coverage
```

### 10.3 No Run-While-Compiling

Nx86 MUST NOT default to running titles while AOT compilation continues.

Nx86 is not trying to be an instant-boot emulator.

The user-facing model is:

```text
Compile first.
Then play.
```

---

## 11. Game Import and Library Model

### 11.1 Import Sources

Nx86 MUST support:

1. importing a file
2. importing a folder
3. scanning a library folder
4. manual import
5. automatic library rescans

### 11.2 Storage Modes

Nx86 SHOULD allow:

1. reference in place
2. copy into Nx86 folder
3. move into Nx86 folder

AOT output, cache data, profile data, shader data, and reports MUST live in Nx86-managed storage.

### 11.3 Unknown Inputs

Unknown or invalid files MUST be refused until v2.

The v1-era importer should avoid ambiguous “maybe valid” imports.

### 11.4 Inspector Behavior

The Inspector MAY inspect a title even if the title cannot compile.

If the title can run only partially, Nx86 MAY use JIT to run it as compiled as possible.

### 11.5 Title Database

Nx86 MUST maintain an internal title database.

The database SHOULD use:

```text
SQLite + human-readable sidecar files
```

SQLite handles:

1. indexing
2. title lookup
3. cache lookup
4. update tracking
5. profile tracking
6. crash report indexing
7. compatibility history
8. shader cache indexing

Sidecar files handle:

1. readable metadata
2. per-title settings
3. compatibility notes
4. compile reports
5. profile summaries
6. title profile config
7. developer notes

---

## 12. Title Folder Layout

Nx86 organizes games by title ID.

Example:

```text
Nx86/
├── titles/
│   └── 0100ABCD12345678/
│       ├── title.nxmeta
│       ├── settings.toml
│       ├── legal.txt
│       ├── content/
│       ├── versions/
│       ├── updates/
│       ├── dlc/
│       ├── cache/
│       │   ├── cpu/
│       │   ├── jit-promoted/
│       │   ├── shaders/
│       │   ├── pipelines/
│       │   └── rollback/
│       ├── profiles/
│       ├── reports/
│       ├── logs/
│       ├── crash/
│       └── inspector/
├── database/
├── shared-profiles/
├── global-cache/
└── config/
```

Each title folder SHOULD contain:

1. title metadata
2. user settings
3. version metadata
4. update metadata
5. DLC metadata
6. CPU native cache
7. promoted JIT cache
8. shader cache
9. pipeline cache
10. runtime profiles
11. compile reports
12. crash reports
13. logs
14. inspector exports
15. legal warnings if content is copied

---

## 13. Process Architecture

Nx86 SHOULD use a multi-process architecture.

### 13.1 GUI Process

The GUI process handles:

1. library UI
2. title settings
3. compile progress
4. runtime status
5. profile management
6. cache management
7. crash display
8. user input to the app
9. process supervision

The GUI process SHOULD NOT directly execute guest game code.

### 13.2 Compiler Worker Process

The compiler SHOULD run in a separate worker process.

Reasons:

1. compiler crashes do not kill GUI
2. memory usage is isolated
3. long tasks can be restarted
4. checkpointing is easier
5. progress updates can stream over IPC
6. future multi-worker compilation becomes easier

### 13.3 Runtime Process

Each launched title MUST run in an isolated runtime process/state.

Reasons:

1. stable VMM layout
2. better crash isolation
3. no cross-title memory corruption
4. simpler process cleanup
5. cleaner debugging
6. safer native code execution

Nx86 MUST NOT support multiple games running simultaneously.

### 13.4 IPC

Nx86 SHOULD use a structured IPC layer between:

1. GUI process
2. compiler process
3. runtime process
4. profile/indexing process if separated later

IPC messages SHOULD be versioned.

IPC should support:

1. progress events
2. log events
3. crash events
4. compile task commands
5. runtime launch commands
6. profile upload/download commands
7. cache operations
8. cancellation
9. pause/resume

Example IPC event:

```text
CompileProgress {
    title_id,
    phase,
    percent,
    current_module,
    functions_discovered,
    functions_compiled,
    native_coverage_estimate,
    cache_size_bytes
}
```

---

## 14. Compilation Model

Nx86 uses compile-before-play.

### 14.1 Initial Compilation

Initial compilation MUST:

1. scan the title
2. identify executable modules
3. hash executable pages
4. decode AArch64
5. recover control flow
6. lift to NxIR
7. verify NxIR
8. optimize NxIR
9. generate x86_64-v4 native code
10. emit many small native objects
11. generate guard/deopt metadata
12. compile shaders where possible
13. prepare pipeline cache where possible
14. write cache metadata
15. write compile report
16. estimate Native Coverage

### 14.2 Threading

The compiler SHOULD use all CPU cores by default.

Nx86 MUST warn the user before doing so.

The user MUST be able to cap compile thread count.

### 14.3 Pause/Resume

Compilation MUST be pausable.

Compilation MUST be resumable.

Compilation SHOULD checkpoint:

1. title scan state
2. hash state
3. decoded instruction ranges
4. discovered functions
5. CFG recovery
6. NxIR lifting state
7. optimization state
8. emitted object state
9. shader compilation state
10. profile ingestion state
11. verification state

### 14.4 Compilation Artifacts

Compilation may produce:

1. decoded instruction database
2. function table
3. CFG table
4. NxIR dumps in debug/research
5. native objects
6. relocation tables
7. guard tables
8. deopt tables
9. code maps
10. shader objects
11. pipeline cache data
12. reports

Optimized NxIR SHOULD NOT be cached by default because native objects are the main persistent output.

---

## 15. Native Coverage

Native Coverage is the main progress metric.

### 15.1 Meaning

Native Coverage measures how much functional game code and GPU/shader/pipeline work has been compiled and prepared for use outside the full Nx86 emulator environment.

It is not merely “bytes compiled.”

Native Coverage answers:

```text
How close is this title to being usable as Pure AOT native output?
```

### 15.2 Coverage Categories

Nx86 SHOULD track:

1. CPU static coverage
2. CPU executed coverage
3. shader readiness
4. pipeline readiness
5. fastmem coverage
6. slowmem count
7. runtime helper count
8. JIT fallback count
9. deopt count
10. interpreter usage

### 15.3 User-Facing Coverage

The main user-facing percentage SHOULD represent functional readiness toward Pure AOT.

Developer views MAY show detailed sub-metrics.

### 15.4 Coverage Bands

| Coverage | Label |
|---:|---|
| 0.00% – 59.99% | Terrible |
| 60.00% – 89.99% | Poor |
| 90.00% – 97.99% | Great |
| 98.00% – 99.99% | Excellent |
| 100.00% | Perfect |

### 15.5 Perfect

Perfect requires:

1. 100% required CPU native readiness
2. 100% shader/pipeline readiness
3. no JIT requirement
4. no interpreter requirement
5. no unresolved runtime code paths
6. no slowmem-dependent game logic paths that prevent Pure AOT
7. valid Pure AOT folder generation

---

## 16. Fastmem and Slowmem

### 16.1 Fastmem

Fastmem is the ideal memory path.

Fastmem means a guest memory operation becomes a direct native host memory operation.

Conceptual path:

```text
guest load/store
  ↓
native x86 load/store
```

Fastmem is considered native-ready.

### 16.2 Slowmem

Slowmem is runtime-checked memory access.

Conceptual path:

```text
guest load/store
  ↓
call Nx86 memory helper
  ↓
check page tables/permissions/MMIO/state
  ↓
perform access or fault
```

Slowmem handles:

1. MMIO
2. protected pages
3. unmapped pages
4. page-boundary edge cases
5. debug checks
6. poison checks
7. invalid access reports
8. special service regions

Slowmem counts against Native Coverage because it depends on Nx86 runtime helper logic rather than pure native direct execution.

### 16.3 Slowmem Reporting

Nx86 SHOULD report:

1. slowmem call count
2. slowmem hot addresses
3. slowmem source blocks
4. slowmem reason
5. slowmem-to-fastmem promotion opportunities

---

## 17. Pure AOT

Pure AOT is the hidden endgame.

Pure AOT does not exist until 100% readiness is possible.

### 17.1 Definition

Pure AOT means:

```text
No JIT.
No interpreter.
No full Nx86 emulator application required.
Native PC executable/folder output.
```

### 17.2 Current Pure AOT Goal

The current goal is:

```text
folder with native code, copied local content, config, shaders, minimal runtime libraries, and launcher
```

### 17.3 Future Pure AOT Goal

The future goal is:

```text
true PC-port-style executable
```

### 17.4 Runtime Libraries

Pure AOT may still require bundled native Nx86 runtime libraries.

Examples:

1. input/gamepad library
2. windowing library
3. GPU runtime library
4. audio library
5. filesystem shim
6. service replacement library
7. per-title special behavior library

These libraries replace the need for the full Nx86 emulator app.

### 17.5 Pure AOT Folder Layout

Example:

```text
Nx86PureAOT/
└── 0100ABCD12345678/
    ├── legal.txt
    ├── launch.sh
    ├── title.nxmeta
    ├── bin/
    │   ├── game
    │   ├── libnx86_runtime.so
    │   ├── libnx86_input.so
    │   ├── libnx86_gpu.so
    │   ├── libnx86_audio.so
    │   └── libnx86_services.so
    ├── native/
    │   ├── main.so
    │   ├── sdk.so
    │   ├── subsdk0.so
    │   └── objects/
    ├── content/
    ├── shaders/
    ├── pipelines/
    ├── config/
    ├── profiles/
    └── reports/
```

### 17.6 Pure AOT Requirements

Pure AOT MUST require:

1. 100% Native Coverage
2. no JIT
3. no interpreter
4. shader/pipeline readiness
5. no unresolved guest code
6. no unresolved runtime code path
7. bundled minimal runtime libraries
8. legal.txt
9. validated content layout
10. reproducible launch

Pure AOT SHOULD be hidden under advanced per-title settings.

---

## 18. Continuous Dynamic Compilation

Continuous Dynamic Compilation is the core Nx86 technique.

### 18.1 Lifecycle

```text
Initial compile
  ↓
Launch
  ↓
Runtime profile
  ↓
Emergency JIT if needed
  ↓
Log missing paths
  ↓
Promote discoveries
  ↓
Passive rebuild
  ↓
Higher Native Coverage
```

### 18.2 Runtime Feedback

Nx86 records:

1. indirect branch targets
2. hidden entrypoints
3. guard failures
4. deopt events
5. emergency JIT blocks
6. runtime helper calls
7. slowmem events
8. executable writes
9. scheduler events
10. crashes
11. shader usage
12. pipeline usage
13. hot blocks
14. hot traces
15. compatibility patches used

### 18.3 Passive Rebuild

Passive rebuild SHOULD:

1. read runtime profiles
2. sanitize profile data
3. identify new AOT candidates
4. promote JIT blocks
5. improve guards
6. compile missing shader/pipeline data
7. reduce slowmem use
8. rebuild affected objects
9. improve Native Coverage
10. write reports

---

## 19. JIT Policy

JIT is allowed but not primary.

### 19.1 Emergency JIT Unit

Emergency JIT initially compiles:

```text
single basic blocks
```

This prioritizes compile speed.

### 19.2 JIT Optimization

JIT uses the same IR/backend pipeline as AOT.

JIT prioritizes compile speed over maximum optimization.

### 19.3 JIT Patching

JIT MAY patch AOT code.

Patching MUST happen at safe points.

Safe points include:

1. block boundaries
2. frame boundaries
3. deopt boundaries
4. runtime service boundaries
5. scheduler yield points

### 19.4 JIT Visibility

JIT activity SHOULD be visible in an easy-to-access developer overlay.

Casual users should not be spammed with JIT activity.

---

## 20. Deoptimization

Deoptimization allows speculative native code to recover when assumptions fail.

### 20.1 Rule

Nx86 MUST enforce:

```text
No speculation without deopt.
No guard without recovery.
```

### 20.2 Deopt State

Deopt MUST reconstruct complete guest-visible state.

This includes:

1. general registers
2. SP
3. PC
4. NZCV flags
5. lazy flag state
6. FP registers
7. SIMD registers
8. FPCR
9. FPSR
10. thread state
11. pending exception state
12. service call state
13. memory side-effect boundary
14. active guard metadata

### 20.3 Deopt Metadata

Deopt metadata SHOULD exist:

1. inside native object files
2. in sidecar files

Object metadata is for runtime.  
Sidecar metadata is for GUI/debugging.

### 20.4 Deopt Failure

If deopt fails, Nx86 MUST crash loudly.

It MUST NOT continue with corrupted state.

---

## 21. NxIR

NxIR is Nx86’s intermediate representation.

### 21.1 Design Requirements

NxIR MUST:

1. be close to AArch64 semantics
2. preserve guest instruction boundaries
3. support high-level operations
4. track side effects explicitly
5. support lazy flags
6. support atomics
7. support barriers
8. support FP/SIMD
9. support guards
10. support deopt points
11. support runtime helpers
12. support debug annotations
13. support serialization
14. support verification

### 21.2 Instruction Boundaries

NxIR MUST preserve guest instruction boundaries for:

1. debugging
2. crash reports
3. deopt
4. differential testing
5. GUI inspection
6. native mapping
7. profile feedback
8. executable invalidation

### 21.3 High-Level Operations

NxIR SHOULD support high-level operations such as:

1. memcpy
2. memmove
3. memset
4. atomic wait
5. atomic wake
6. service call
7. shader dispatch metadata
8. runtime helper call
9. sqrt
10. vector shuffle
11. memory fence
12. block guard
13. deopt checkpoint

### 21.4 Side Effects

NxIR MUST model side effects including:

1. memory reads
2. memory writes
3. atomics
4. barriers
5. service calls
6. MMIO
7. exceptions
8. FP status changes
9. thread synchronization
10. executable page writes
11. guard dependencies
12. deopt boundaries

### 21.5 Verifier

The NxIR verifier MUST run after every optimization pass in debug and research modes.

It SHOULD be stripped from release builds.

The verifier checks:

1. SSA correctness
2. type correctness
3. side-effect ordering
4. legal terminators
5. block validity
6. guard/deopt validity
7. instruction boundary mapping
8. memory dependency validity
9. backend-lowerable forms

---

## 22. AArch64 Frontend

### 22.1 Decoder

The decoder MUST:

1. decode AArch64 instructions
2. classify instruction groups
3. provide disassembly
4. expose instruction metadata
5. support fuzzing
6. support exact byte-to-instruction mapping

### 22.2 Lifter

The lifter MUST:

1. convert decoded instructions to NxIR
2. preserve instruction boundaries
3. emit side effects
4. emit lazy flags
5. emit memory operations
6. emit atomics
7. emit barriers
8. emit FP/SIMD operations
9. emit system instruction behavior
10. emit traps/helpers for unsupported cases

### 22.3 Required Instruction Categories

Nx86 MUST eventually support:

1. integer arithmetic
2. logical operations
3. branches
4. calls
5. returns
6. indirect branches
7. loads/stores
8. paired loads/stores
9. atomics
10. barriers
11. FP scalar
12. NEON/SIMD
13. system instructions
14. exception-related behavior
15. service call boundaries

### 22.4 Accuracy

Nx86 MUST aim for exact guest-visible behavior.

Floating-point behavior MUST aim for bit-perfect Switch behavior.

Host math shortcuts MUST be disabled in accuracy mode unless proven exact.

---

## 23. Custom x86_64-v4 Backend

Nx86 uses a custom backend.

### 23.1 Backend Pipeline

```text
NxIR
  ↓
legalization
  ↓
instruction selection
  ↓
register allocation
  ↓
machine optimization
  ↓
hot/cold splitting
  ↓
block layout
  ↓
internal assembler
  ↓
machine code encoding
  ↓
relocation
  ↓
object/JIT emission
```

### 23.2 Register Allocation

The first register allocator SHOULD be simple.

The long-term allocator SHOULD become advanced and profile-guided.

Backend priority:

```text
fastest generated code
size be damned
```

### 23.3 Internal Assembler

Nx86 MUST include an integrated x86_64 assembler.

The assembler MUST support:

1. x86_64 encoding
2. x86_64-v4 instructions
3. vector instructions
4. mask registers
5. labels
6. relocations
7. patch sites
8. guard patching
9. block chain patching
10. native dumps
11. debug/unwind metadata hooks

### 23.4 Hot/Cold Splitting

The backend MUST support hot/cold splitting.

Cold code SHOULD NOT automatically receive lower optimization.

### 23.5 Native Dumps

Debug builds SHOULD emit human-readable assembly dumps.

Native dumps SHOULD be deleted after successful runs unless preservation is requested.

---

## 24. Native Object Format

Nx86 emits many small native objects.

### 24.1 Object Contents

Native objects SHOULD contain:

1. object header
2. compiler version
3. backend version
4. CPU target
5. title ID
6. title version
7. executable hash dependencies
8. code bytes
9. relocation table
10. block table
11. function table
12. guest-to-native map
13. native-to-guest map
14. guard table
15. deopt table
16. page dependency table
17. hot/cold layout
18. debug/unwind info
19. validation hash
20. profile counters

### 24.2 Loading

Objects MAY be:

1. memory-mapped from disk
2. loaded into RAM
3. handled differently depending on mode

### 24.3 Interdependence

Native objects do not need to be standalone.

They may depend on:

1. other native objects
2. runtime libraries
3. metadata tables
4. relocation tables
5. service thunks
6. shader/pipeline state
7. profile data

---

## 25. Virtual Memory Manager

Nx86 includes a VMM from day one.

### 25.1 Arena

Nx86 MUST reserve a 64 GB virtual address arena at startup.

The arena supports:

1. guest memory
2. fastmem
3. mirrored memory regions
4. codegen assumptions
5. stable address layout

### 25.2 Features

The VMM MUST support:

1. guest virtual memory
2. page mapping
3. page unmapping
4. page permissions
5. host page protections
6. software permission tables
7. fastmem
8. slowmem
9. memory mirroring
10. guard pages
11. MMIO explicit checks
12. executable page hashes
13. executable page generations
14. self-modifying code detection
15. invalidation callbacks
16. debug poisoning
17. thread-safe updates
18. crash reports

### 25.3 Memory Mirroring

Memory mirroring SHOULD be enabled in release mode.

Memory mirroring SHOULD be disabled in debug mode.

### 25.4 VMM Faults

VMM faults MUST pause execution and show a crash report.

Crash reports SHOULD include:

1. faulting address
2. access type
3. guest PC
4. guest thread
5. source block
6. call stack
7. native object
8. page permissions
9. page generation
10. recent executable writes
11. scheduler context

---

## 26. Self-Modifying Code

Nx86 MUST support self-modifying code.

When guest code writes to executable memory, Nx86 MUST:

1. log the write
2. include call stack
3. include source block
4. mark page dirty
5. increment page generation
6. invalidate dependent objects
7. remove unsafe block chains
8. route future execution through dispatcher/JIT
9. feed event into profile data

Self-modifying code MUST NOT trigger visible gameplay warnings by default.

It SHOULD be visible in developer tools and crash reports.

---

## 27. Threading and Scheduling

Nx86 supports guest threading from day one.

### 27.1 Thread Models

Nx86 SHOULD support:

1. 1:1 guest-to-host threads
2. optional fiber/task mode
3. deterministic developer scheduler
4. crash-window replay logs

### 27.2 GUI Thread View

The GUI SHOULD show:

1. active guest threads
2. per-thread CPU usage
3. thread state
4. current block/function if available
5. scheduler status in developer mode

### 27.3 Replay Logs

Replay logs SHOULD capture crash windows.

Replay logs MUST have a size cap.

Replay logs SHOULD be analyzed to improve future rebuilds.

### 27.4 Auto-Parallelization

Auto-parallelization is experimental.

It MUST be per-title opt-in.

It MAY change frame timing.

It MUST NOT be enabled globally by default.

---

## 28. Atomics and Barriers

Nx86 MUST implement atomics correctly from day one.

### 28.1 Atomics

Atomic support includes:

1. exclusive loads
2. exclusive stores
3. acquire loads
4. release stores
5. compare-and-swap behavior
6. atomic read-modify-write
7. failure behavior
8. thread-visible ordering
9. page fault interactions
10. deopt interactions

### 28.2 Barriers

Barriers MUST be optimized semantically.

Nx86 MUST NOT blindly delete barriers.

Barrier lowering must consider:

1. normal memory
2. MMIO
3. service boundaries
4. executable memory
5. self-modifying code
6. thread synchronization
7. debug validation
8. replay determinism

---

## 29. Guarded Speculative Optimization

GSO means:

```text
Guarded Speculative Optimization
```

### 29.1 Purpose

GSO allows Nx86 to:

1. predict indirect branch targets
2. optimize vtable dispatch
3. optimize callback paths
4. specialize hot callsites
5. inline likely targets
6. reduce dispatcher overhead
7. patch fast paths
8. deopt safely when wrong
9. use runtime feedback to improve future AOT

### 29.2 GSO Profiles

GSO profiles SHOULD be auto-generated.

Advanced developers MAY create manual overrides.

### 29.3 Official GSO Code

Official GSO code MAY be compiled into Nx86.

Official GSO code MUST:

1. be original code
2. avoid copyrighted code
3. be reviewed
4. be documented
5. be linked to title/version/hash when title-specific
6. improve performance, fix bugs, or allow title execution
7. be safe enough for official inclusion

---

## 30. Title Behavior Patches

Nx86 supports title behavior patches.

Behavior patches may:

1. improve compatibility
2. fix title-specific bugs
3. allow titles to run
4. implement runtime-level FPS unlock
5. patch service behavior
6. patch scheduling behavior
7. patch GPU behavior

Required compatibility patches MAY be enabled by default and undisableable.

Built-in required patches do not lower compatibility status.

---

## 31. Profiles and Sharing

### 31.1 Sharing Policy

Profile sharing is opt-in.

Profile downloads require user approval.

Profile uploads ask every time.

Verified profiles SHOULD be signed.

Profile distribution MAY be centralized, decentralized, or both.

### 31.2 Profile Data

Profiles may contain:

1. title ID
2. title version
3. executable hashes
4. branch target addresses
5. hot block data
6. guard data
7. shader hashes
8. pipeline hashes
9. performance ratings by hardware class
10. sanitized crash signatures
11. GSO suggestions

Profiles MUST NOT contain:

1. game assets
2. copied executable bytes
3. binary blobs
4. save data
5. screenshots
6. personal paths
7. usernames
8. personal data
9. memory dumps containing copyrighted data

### 31.3 Sanitizer

Nx86 MUST include a profile sanitizer.

The sanitizer MUST reject or strip:

1. binary blobs
2. copied game bytes
3. assets
4. saves
5. screenshots
6. personal paths
7. personal identifiers
8. suspicious payloads
9. raw memory dumps

---

## 32. Compatibility Database

Nx86 compatibility status is based on:

1. user reports
2. official testing
3. automated profile data
4. Native Coverage
5. crash reports
6. performance reports

Compatibility and Native Coverage may be combined.

Perfect compatibility MUST require 100% Native Coverage.

Built-in required patches do not lower compatibility status.

---

## 33. Graphics
### Vulkan Binding Choice

Nx86 SHOULD use `ash` for the Vulkan backend.

`ash` is preferred because Nx86 requires low-level Vulkan control for shader translation, pipeline cache management, descriptor layout control, synchronization, command buffer recording, memory management, debug tooling, and future vendor-specific optimization.

Nx86 MUST wrap `ash` behind an internal `nx86-vulkan` abstraction. Raw Vulkan handles and unsafe Vulkan calls SHOULD NOT leak into higher-level emulator crates.

Higher-level graphics crates such as `wgpu` or `vulkano` are not preferred for the core renderer because Nx86 needs explicit Vulkan behavior rather than a portability abstraction.

### 33.1 Backend

Initial backend:

```text
Vulkan
```

Future backend:

```text
D3D12
```

### 33.2 Shader AOT

Shader AOT happens during initial compile.

Shared profiles MAY assist shader AOT.

Shader data SHOULD live in the title folder, separate from CPU cache.

### 33.3 GPU Coverage

Nx86 does not show separate GPU coverage by default.

GPU readiness is included in Native Coverage.

Developer tools MAY show shader and pipeline breakdowns.

### 33.4 FPS and Resolution

Nx86 supports:

1. runtime-level FPS unlock
2. per-title FPS behavior
3. global resolution scaling
4. per-title resolution overrides

---

## 34. Runtime Libraries

Pure AOT may require bundled runtime libraries.

Libraries may provide:

1. gamepad support
2. input translation
3. windowing
4. audio
5. graphics
6. filesystem shims
7. service implementations
8. title-specific compatibility services

These libraries replace the need for the full Nx86 emulator app.

---

## 35. Inspector

Nx86 includes an Inspector.

The Inspector displays:

1. title structure
2. title ID
3. versions
4. updates
5. DLC
6. executable modules
7. sections
8. disassembly
9. recovered functions
10. recovered CFG
11. NxIR
12. native mapping
13. guards
14. deopt points
15. Native Coverage
16. profile events
17. executable write logs
18. shader/pipeline data

---

## 36. GUI

### 36.1 Main Screens

Nx86 SHOULD include:

1. First Launch Wizard
2. Library
3. Title Details
4. Compile Screen
5. Runtime Screen
6. Settings
7. Cache Manager
8. Profile Manager
9. Inspector
10. Disassembler
11. Developer Tools
12. Crash Analyzer
13. Compatibility Reports

### 36.2 Compile Screen

The compile screen is visual-first.

It shows compact logs with a “see more” button.

It SHOULD display:

1. current phase
2. progress
3. Native Coverage estimate
4. code objects generated
5. functions discovered
6. functions compiled
7. guards inserted
8. deopt points
9. shader AOT status
10. cache size
11. warnings
12. compact logs

### 36.3 Developer Overlay

The developer overlay SHOULD show:

1. AOT activity
2. emergency JIT events
3. Native Coverage
4. slowmem events
5. runtime helpers
6. guest threads
7. per-thread CPU usage
8. shader/pipeline events
9. guard failures
10. deopt events

---

## 37. Telemetry and Privacy

Nx86 supports opt-in sanitized telemetry/profile sharing.

Nx86 MUST:

1. never upload automatically without consent
2. ask every time before uploading
3. sanitize before export
4. show what will be shared
5. avoid personal data
6. avoid copyrighted blobs
7. avoid saves
8. allow disabling sharing entirely

---

## 38. Testing

Nx86 requires a serious test suite.

### 38.1 Test Targets

Tests should run on:

1. synthetic ARM64 environment
2. Nx86 interpreter
3. Nx86 JIT
4. Nx86 AOT
5. optimized Nx86 AOT
6. homebrew on real hardware where possible

### 38.2 Test Categories

Test categories include:

1. integer arithmetic
2. flags
3. branches
4. calls
5. indirect branches
6. loads/stores
7. unaligned memory
8. atomics
9. barriers
10. FP
11. SIMD
12. system instructions
13. service calls
14. VMM faults
15. self-modifying code
16. threading
17. scheduler replay
18. shader AOT
19. pipeline cache
20. Pure AOT folder behavior

### 38.3 Differential Testing

Nx86 SHOULD compare:

```text
interpreter result
JIT result
AOT result
optimized AOT result
real hardware result where available
```

Compared state includes:

1. general registers
2. flags
3. vector registers
4. FP status
5. memory
6. thread-visible behavior
7. service behavior
8. exception behavior

---

## 39. Research Direction

Nx86 development should include ongoing research into:

1. Arm memory model
2. AArch64 barriers
3. AArch64 SIMD/FP behavior
4. emulator JIT designs
5. binary translation
6. deoptimization systems
7. JavaScript VM optimization strategies
8. profile-guided optimization
9. Vulkan pipeline caching
10. shader translation
11. x86_64-v4 codegen
12. AVX-512 register allocation
13. fastmem/slowmem designs
14. scheduler replay systems
15. self-modifying code invalidation

Research findings should be folded back into the spec.

---

## 40. Repository Layout

Nx86 uses a Rust monorepo.

Suggested layout:

```text
nx86/
├── README.md
├── SPEC.md
├── Cargo.toml
├── crates/
│   ├── nx86-app/
│   ├── nx86-gui/
│   ├── nx86-core/
│   ├── nx86-title-db/
│   ├── nx86-import/
│   ├── nx86-inspector/
│   ├── nx86-arm64-decode/
│   ├── nx86-arm64-lift/
│   ├── nx86-ir/
│   ├── nx86-ir-opt/
│   ├── nx86-backend/
│   ├── nx86-x64-asm/
│   ├── nx86-x64-v4/
│   ├── nx86-regalloc/
│   ├── nx86-object/
│   ├── nx86-jit/
│   ├── nx86-runtime/
│   ├── nx86-vmm/
│   ├── nx86-scheduler/
│   ├── nx86-hle/
│   ├── nx86-input/
│   ├── nx86-audio/
│   ├── nx86-gpu/
│   ├── nx86-vulkan/
│   ├── nx86-shader/
│   ├── nx86-profile/
│   ├── nx86-gso/
│   ├── nx86-title-profiles/
│   ├── nx86-cache/
│   ├── nx86-debug/
│   ├── nx86-testsuite/
│   └── nx86-fuzz/
├── docs/
│   ├── phases/
│   ├── research/
│   ├── adr/
│   └── developer/
└── tools/
```

---

## 41. Issue Templates

Nx86 should include issue templates for:

1. compiler bug
2. backend bug
3. runtime bug
4. game compatibility bug
5. shader/graphics bug
6. profile bug
7. crash report
8. performance issue
9. documentation issue
10. feature request

---

## 42. Expanded Roadmap

This roadmap is intentionally granular.

Each phase should produce a working, testable step.

### Phase 0: Research and Spec Foundation

Goals:

1. define project goals
2. define terminology
3. research Arm semantics
4. research x86_64-v4 codegen
5. research Vulkan pipeline behavior
6. research emulator memory models
7. write SPEC.md
8. write README.md skeleton

Exit criteria:

1. SPEC.md exists
2. README.md exists
3. crate plan exists
4. first prototype target is defined

---

### Phase 1: Repository Bootstrap

Goals:

1. create Rust workspace
2. add crate skeletons
3. add formatting/lint config
4. add CI placeholder
5. add issue templates
6. add docs folder
7. add basic logging crate

Exit criteria:

1. workspace builds
2. empty app launches
3. CI can compile empty project

---

### Phase 2: GUI Shell

Goals:

1. create egui app
2. create main window
3. create theme system
4. create navigation shell
5. create empty Library screen
6. create empty Settings screen
7. create empty Compile screen

Exit criteria:

1. Nx86 opens a GUI window
2. navigation works
3. settings persist

---

### Phase 3: First-Launch Wizard

Goals:

1. library folder picker
2. cache folder picker
3. profile folder picker
4. CPU target selection
5. compile thread cap
6. all-core warning
7. profile sharing opt-in
8. default graphics backend selection

Exit criteria:

1. wizard runs on first launch
2. config is saved
3. app boots into Library after wizard

---

### Phase 4: App Storage and Title Database

Goals:

1. SQLite database setup
2. sidecar metadata system
3. title folder creation
4. cache folder creation
5. profile folder creation
6. migration system

Exit criteria:

1. Nx86 can create and list title entries
2. title folders are deterministic
3. database survives restart

---

### Phase 5: Job System and IPC Foundation

Goals:

1. background job model
2. compiler worker process skeleton
3. runtime process skeleton
4. IPC message format
5. progress events
6. cancellation events

Exit criteria:

1. GUI can launch worker process
2. worker sends progress events
3. GUI displays progress

---

### Phase 6: Synthetic ARM64 Test Format

Goals:

1. define simple synthetic test file format
2. support raw ARM64 bytes
3. support expected register output
4. support expected memory output
5. support tiny metadata header

Exit criteria:

1. synthetic test files can be loaded
2. expected results can be displayed

---

### Phase 7: CpuState and Guest State Model

Goals:

1. define general registers
2. define SP/PC
3. define NZCV
4. define FP/SIMD registers
5. define FPCR/FPSR
6. define thread-local state
7. define serialization for debug

Exit criteria:

1. CpuState can be created
2. CpuState can be dumped
3. test harness can compare CpuState

---

### Phase 8: AArch64 Decoder Skeleton

Goals:

1. decode small instruction subset
2. print disassembly
3. classify instruction types
4. expose raw bytes
5. support decoder errors

Exit criteria:

1. MOV/ADD/SUB/B/SVC decode
2. GUI can show decoded instructions

---

### Phase 9: Tiny Interpreter

Goals:

1. execute tiny integer subset
2. update registers
3. update PC
4. handle basic branch
5. handle halt/SVC test exit

Exit criteria:

1. synthetic ARM64 add program executes
2. expected register values match

---

### Phase 10: VMM Skeleton

Goals:

1. reserve 64 GB arena
2. create page table structure
3. map/unmap pages
4. implement read/write helpers
5. implement debug memory dump

Exit criteria:

1. synthetic tests can read/write guest memory
2. VMM faults produce errors

---

### Phase 11: Simple Drawing Demo

Goals:

1. define synthetic framebuffer region
2. allow ARM64 test to write pixels
3. display framebuffer in GUI
4. support simple color output

Exit criteria:

1. ARM64 synthetic program draws something simple

---

### Phase 12: NxIR Core

Goals:

1. define IR module/function/block
2. define values/types
3. define integer ops
4. define branch ops
5. define memory ops
6. preserve instruction boundaries

Exit criteria:

1. decoded ARM64 can be lifted into simple NxIR

---

### Phase 13: NxIR Verifier

Goals:

1. SSA checks
2. type checks
3. block terminator checks
4. side-effect checks
5. instruction boundary checks

Exit criteria:

1. verifier catches invalid IR
2. verifier runs after lifting

---

### Phase 14: Core Integer Lifter

Goals:

1. lift MOV
2. lift ADD/SUB
3. lift logical ops
4. lift branches
5. lift loads/stores
6. lift SVC test exit

Exit criteria:

1. synthetic integer programs lift to NxIR
2. interpreter and NxIR semantics agree

---

### Phase 15: Lazy Flags Model

Goals:

1. define lazy NZCV expressions
2. lift ADDS/SUBS/CMP
3. eliminate unused flags
4. materialize flags when needed

Exit criteria:

1. condition branches work through lazy flags
2. tests cover overwritten flags

---

### Phase 16: x86_64 Assembler Skeleton

Goals:

1. internal assembler API
2. labels
3. basic mov/add/sub/cmp/jmp
4. function prologue/epilogue
5. code buffer
6. disassembly dump placeholder

Exit criteria:

1. assembler emits callable x86_64 code

---

### Phase 17: Executable Memory Manager

Goals:

1. allocate executable memory
2. write code
3. set permissions
4. free safely
5. support code pointers

Exit criteria:

1. generated native function can be called from Rust

---

### Phase 18: First Native Block

Goals:

1. lower tiny NxIR block to x86_64
2. call native block
3. compare result with interpreter
4. show success in GUI

Exit criteria:

1. ARM64 add program runs as native x86_64 code

---

### Phase 19: Basic Register Allocator

Goals:

1. implement simple allocator
2. handle spills
3. map guest values
4. handle block-local temporaries

Exit criteria:

1. multi-instruction native blocks run correctly

---

### Phase 20: AOT Object Format v0

Goals:

1. define `.nxo` object header
2. store code bytes
3. store guest mapping
4. store validation hash
5. load object from disk

Exit criteria:

1. compiled block persists across restart

---

### Phase 21: Cache Manager v0

Goals:

1. cache manifest
2. shallow check
3. full check placeholder
4. cache deletion UI
5. cache size accounting

Exit criteria:

1. GUI shows cache status
2. compiled object loads from cache

---

### Phase 22: Dispatcher

Goals:

1. guest PC lookup
2. native block call
3. block exit handling
4. branch target routing
5. missing block handling

Exit criteria:

1. multiple native blocks execute through dispatcher

---

### Phase 23: Emergency JIT v0

Goals:

1. compile missing basic block
2. insert into code cache
3. log JIT event
4. continue execution

Exit criteria:

1. missing block can be JITed and executed

---

### Phase 24: Runtime Profile Logging

Goals:

1. profile file format
2. log JIT blocks
3. log branch targets
4. log helper calls
5. log slowmem events

Exit criteria:

1. runtime produces profile data

---

### Phase 25: Profile-Guided Rebuild v0

Goals:

1. read runtime profile
2. identify JIT block candidates
3. promote block to AOT object
4. update Native Coverage

Exit criteria:

1. second run uses promoted native object

---

### Phase 26: CFG Recovery v0

Goals:

1. recursive traversal
2. direct branch following
3. function candidate discovery
4. basic block table
5. CFG display

Exit criteria:

1. synthetic programs produce CFG

---

### Phase 27: Inspector v0

Goals:

1. disassembly view
2. function list
3. block list
4. NxIR view
5. native mapping view

Exit criteria:

1. user can inspect synthetic program

---

### Phase 28: Guard and Deopt Metadata

Goals:

1. define guard IR op
2. define deopt point
3. store deopt metadata
4. handle guard failure
5. crash on deopt failure

Exit criteria:

1. failed guard routes to deopt handler

---

### Phase 29: Block Chaining v0

Goals:

1. direct chaining
2. table chaining
3. invalidation hooks
4. disable in debug

Exit criteria:

1. hot loop executes with block chaining

---

### Phase 30: Fastmem v0

Goals:

1. direct memory lowering
2. memory base register
3. fast load/store
4. fallback to slowmem

Exit criteria:

1. synthetic memory tests run through fastmem

---

### Phase 31: Slowmem v0

Goals:

1. runtime load/store helpers
2. permission checks
3. fault reporting
4. slowmem counters
5. Native Coverage penalty

Exit criteria:

1. invalid access pauses and shows crash report

---

### Phase 32: Memory Mirroring

Goals:

1. mirrored region design
2. release-mode enablement
3. debug-mode disablement
4. correctness tests

Exit criteria:

1. release can use mirrored fast paths

---

### Phase 33: Self-Modifying Code Support

Goals:

1. executable page write detection
2. page generation increment
3. object invalidation
4. chain invalidation
5. profile event logging

Exit criteria:

1. synthetic self-modifying test invalidates old code

---

### Phase 34: Atomics v0

Goals:

1. exclusive monitor model
2. LDXR/STXR subset
3. acquire/release subset
4. atomic tests
5. thread interaction tests

Exit criteria:

1. basic atomic synthetic tests pass

---

### Phase 35: Barrier Semantics v0

Goals:

1. represent DMB/DSB/ISB
2. semantic lowering
3. debug validation
4. threading tests

Exit criteria:

1. barriers behave correctly in synthetic tests

---

### Phase 36: Guest Threading v0

Goals:

1. guest thread model
2. host thread mapping
3. thread state
4. thread GUI view
5. per-thread CPU use

Exit criteria:

1. synthetic multi-thread program runs

---

### Phase 37: Scheduler Replay v0

Goals:

1. crash-window replay logs
2. size cap
3. replay metadata
4. developer UI

Exit criteria:

1. crash window can be replay-analyzed

---

### Phase 38: Fiber/Task Mode Prototype

Goals:

1. fiber/task scheduler skeleton
2. optional mode
3. synthetic test support
4. comparison with host threads

Exit criteria:

1. synthetic guest threads can run under fiber mode

---

### Phase 39: FP Scalar Support

Goals:

1. FP registers
2. FP add/sub/mul/div
3. FP comparisons
4. FPCR/FPSR basics
5. exactness tests

Exit criteria:

1. scalar FP synthetic tests pass

---

### Phase 40: SIMD/NEON v0

Goals:

1. vector register model
2. basic NEON integer ops
3. basic NEON FP ops
4. x86_64-v4 lowering
5. debug validation

Exit criteria:

1. basic NEON tests pass

---

### Phase 41: Advanced x86_64-v4 Vector Lowering

Goals:

1. vector register mapping
2. mask register use
3. shuffle lowering
4. compare lowering
5. spill strategy

Exit criteria:

1. NEON-heavy tests run efficiently

---

### Phase 42: Hot/Cold Splitting

Goals:

1. profile hot blocks
2. split cold paths
3. layout hot loops
4. update native mapping

Exit criteria:

1. hot code layout changes based on profile

---

### Phase 43: Native Coverage System

Goals:

1. functional coverage metric
2. static coverage metric
3. executed coverage metric
4. fastmem coverage
5. slowmem penalty
6. GUI display

Exit criteria:

1. compile/runtime screen shows Native Coverage

---

### Phase 44: Homebrew Loader v0

Goals:

1. load simple homebrew
2. basic module metadata
3. entrypoint handling
4. minimal services

Exit criteria:

1. simple homebrew boots

---

### Phase 45: HLE/Service Skeleton

Goals:

1. service call dispatch
2. filesystem skeleton
3. thread service skeleton
4. memory service skeleton
5. input service skeleton

Exit criteria:

1. simple homebrew can call basic services

---

### Phase 46: Input Runtime

Goals:

1. gamepad abstraction
2. keyboard mapping
3. controller config UI
4. runtime input service

Exit criteria:

1. homebrew receives input

---

### Phase 47: Audio Runtime Skeleton

Goals:

1. audio output abstraction
2. buffer model
3. service skeleton
4. timing tests

Exit criteria:

1. simple audio test works

---

### Phase 48: Vulkan Backend Skeleton

Goals:

1. Vulkan device setup
2. swapchain
3. basic frame rendering
4. GUI/runtime separation
5. error handling

Exit criteria:

1. runtime displays a rendered frame

---

### Phase 49: Shader Translation Skeleton

Goals:

1. shader metadata model
2. shader hash model
3. placeholder translation path
4. shader cache folder

Exit criteria:

1. synthetic shader path compiles/caches placeholder

---

### Phase 50: Shader AOT v0

Goals:

1. compile shaders during initial compile
2. store shader cache
3. use shared profile hints
4. report shader readiness

Exit criteria:

1. shader AOT contributes to Native Coverage

---

### Phase 51: Vulkan Pipeline Cache Integration

Goals:

1. pipeline profile format
2. pipeline cache save/load
3. initial compile integration
4. runtime miss logging

Exit criteria:

1. pipeline cache persists across runs

---

### Phase 52: Graphics Profile Feedback

Goals:

1. log shader usage
2. log pipeline usage
3. promote runtime-discovered pipeline data
4. feed passive rebuild

Exit criteria:

1. graphics profile improves next run

---

### Phase 53: Title Behavior Patch System

Goals:

1. patch metadata format
2. built-in required patches
3. undisableable compatibility patches
4. patch reporting

Exit criteria:

1. synthetic title patch modifies behavior intentionally

---

### Phase 54: GSO v0

Goals:

1. guard profile format
2. speculative target metadata
3. auto-generated GSO profile
4. manual local override

Exit criteria:

1. indirect target can become guarded native fast path

---

### Phase 55: Official Title Profiles

Goals:

1. title profile crate
2. profile registry
3. title hash matching
4. compatibility notes
5. GSO integration

Exit criteria:

1. local title profile applies to test title

---

### Phase 56: Profile Sanitizer

Goals:

1. strip personal paths
2. reject blobs
3. reject saves
4. reject screenshots
5. validate shared profile format

Exit criteria:

1. unsafe profile export is blocked

---

### Phase 57: Verified Profile Repository Support

Goals:

1. signed profile format
2. download approval UI
3. upload approval UI
4. decentralized source support
5. hardware class ratings

Exit criteria:

1. sanitized profile can be imported/exported

---

### Phase 58: Compatibility Database

Goals:

1. compatibility labels
2. Native Coverage integration
3. reports
4. title status UI

Exit criteria:

1. title compatibility status displays in library

---

### Phase 59: Commercial Title Import v0

Goals:

1. legal user-provided title import
2. title ID detection
3. update/DLC metadata
4. compile refusal for unsupported cases
5. Inspector support

Exit criteria:

1. commercial title appears in library and Inspector

---

### Phase 60: Commercial Title Boot Experiments

Goals:

1. execute early title startup
2. service logging
3. missing instruction logging
4. JIT emergency path
5. crash reports

Exit criteria:

1. simple commercial title reaches early boot path

---

### Phase 61: Large Title Bring-Up

Goals:

1. improve services
2. improve GPU
3. improve scheduler
4. improve memory behavior
5. improve profile rebuild
6. improve Native Coverage

Exit criteria:

1. complex commercial title reaches visible graphics

---

### Phase 62: TOTK-Class Research Target

Goals:

1. handle large open-world title complexity
2. handle heavy shader/pipeline use
3. handle complex threading
4. handle large memory behavior
5. handle deep profile-guided recompilation

Exit criteria:

1. architecture proves it can target TOTK-class workloads

---

### Phase 63: Maximum Optimization Tier

Goals:

1. aggressive GSO
2. helper inlining
3. deeper superblocks
4. cross-function optimization
5. advanced register allocation
6. huge cache mode

Exit criteria:

1. “I Paid For My Computer” mode produces faster native output

---

### Phase 64: Auto-Parallelization Prototype

Goals:

1. dependency analysis
2. per-title opt-in
3. frame timing warnings
4. synthetic workload split
5. rollback if unsafe

Exit criteria:

1. synthetic single-threaded workload can be split safely

---

### Phase 65: Pure AOT Folder Prototype

Goals:

1. generate folder
2. include native objects
3. include minimal runtime libraries
4. include content copy/reference
5. include legal.txt
6. launch without full GUI

Exit criteria:

1. synthetic title launches from Pure AOT folder

---

### Phase 66: Pure AOT Homebrew

Goals:

1. homebrew Pure AOT folder
2. bundled runtime services
3. shader/pipeline readiness
4. no JIT
5. no interpreter

Exit criteria:

1. homebrew runs without full Nx86 app

---

### Phase 67: Pure AOT Commercial Experiment

Goals:

1. title-specific native folder
2. copied local content
3. minimal runtime libraries
4. compatibility patches
5. no full Nx86 GUI

Exit criteria:

1. selected title launches from generated native folder

---

### Phase 68: x86_64-v3 Backend

Goals:

1. backend target abstraction
2. v3 lowering
3. reduced vector register strategy
4. handheld performance work

Exit criteria:

1. synthetic tests pass on x86_64-v3 backend

---

### Phase 69: Generic x86_64 Backend

Goals:

1. baseline lowering
2. compatibility mode
3. feature detection
4. slower fallback paths

Exit criteria:

1. synthetic tests pass on generic x86_64

---

### Phase 70: Windows Runtime Port

Goals:

1. Windows process model
2. Windows VMM support
3. D3D12 prep
4. Windows input/audio
5. cache path handling

Exit criteria:

1. Nx86 GUI and synthetic tests run on Windows

---

### Phase 71: D3D12 Backend

Goals:

1. D3D12 device setup
2. shader path adaptation
3. pipeline cache equivalent
4. backend selection

Exit criteria:

1. synthetic graphics test runs on D3D12

---

### Phase 72: ARM64 Host Research

Goals:

1. host backend abstraction
2. ARM64 native backend research
3. Apple Silicon/Linux ARM notes
4. future feasibility

Exit criteria:

1. ARM64 host backend plan exists

---

## 43. Final Design Statement

Nx86 is a game-running monster built around native recompilation.

It starts with synthetic ARM64 programs, grows into homebrew support, then targets commercial title compatibility, with TOTK-class workloads as the v1 ambition.

Nx86 accepts long compilation, large caches, aggressive x86_64-v4 code generation, shader AOT, profile sharing, guarded speculation, behavior patches, and deep runtime feedback because its goal is not simply to emulate.

Its goal is:

```text
Compile the game into something that behaves like the original,
runs like native software,
and eventually no longer needs the full emulator around it.
```

Nx86 is not a normal JIT emulator.

Nx86 is Continuous Dynamic Compilation for Switch software.

---
