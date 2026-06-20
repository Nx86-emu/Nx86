# Phase 11: Simple Drawing Demo

Phase 11 lets a synthetic AArch64 program draw into a guest framebuffer. The
narrow decoder gains a 32-bit `STR` (immediate, unsigned offset); the tiny
interpreter now executes against a `nx86-vmm` `GuestMemory` so stores are
observable. Synthetic tests may declare a `[framebuffer]` region (base, width,
height, RGBA8); the runtime maps it, runs the program, and reads the pixels
back. The GUI Tests screen renders the framebuffer as a texture.

Pixels are 32-bit little-endian RGBA words: storing `0xAABBGGRR` lays down the
bytes `RR GG BB AA`.

## Exit Criteria

- A synthetic AArch64 program writes pixels and produces a framebuffer image
  (`tests/synthetic/draw.toml` draws an opaque-blue 2x2 image).
- Stores go through the VMM software page table; expected memory ranges are
  compared against what the program wrote.
- The GUI Tests screen displays the rendered framebuffer.
