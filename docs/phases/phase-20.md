# Phase 20: AOT Object Format v0

Phase 20 gives lowered native blocks a persistent on-disk form. `nx86-object` is
now filled in with `NativeObject` and a compact little-endian `.nxo` format: a
magic + version header, the guest mapping (entry address and exclusive end PC),
the lowerer's stack-frame size, the generated code bytes, and a trailing FNV-1a
content hash.

`NativeObject::to_bytes`/`from_bytes` serialize and validate (magic, version,
exact length, and hash), while `write_to_path`/`read_from_path` are thin
`std::fs` wrappers, and `file_name` keys an object by its guest entry address
(`{entry:016x}.nxo`). The validation hash is a dependency-free integrity and
identity check, not a cryptographic one; the Phase 21 cache "full check" can
upgrade it.

`nx86-backend` bridges the in-memory lowering to the object: `native_object`
builds a `NativeObject` from a `LoweredBlock` and its source function. A
Linux-gated test closes the loop end to end — lower the synthetic add block,
write the object to disk, reload it, allocate executable memory from the loaded
bytes, run it, and confirm the result still matches the interpreter.

The cache manager (manifest, shallow/full checks, size accounting, deletion UI),
the dispatcher, and any storage-layout wiring remain later phases.

## Exit Criteria

- A lowered block serializes to a `.nxo` file and reloads to an identical object.
- Corrupted, truncated, wrong-magic, or wrong-version objects are rejected.
- On Linux x86_64, a block persisted to disk and reloaded executes and matches
  the interpreter — a compiled block persists across restart.
- The object format is pure logic and fully unit-tested on the Apple Silicon dev
  host; only execute-after-reload is host-gated.
