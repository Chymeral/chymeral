<p align="center">
  <img src="branding/chymeral_logo.svg" alt="Chymeral logo" width="192">
</p>

# Chymeral

> A minimalist terminal emulator with a reusable local graphics runtime for
> applications compatible with CLP.

**Chymeral** (official pronunciation: **“Kímeral”**) keeps the normal terminal
model intact while giving compatible programs a direct way to render a retained
2D scene, receive input, and run against a local frame loop. It targets Linux
and Windows. macOS is intentionally outside the current scope.

## What it is

Chymeral is one window with two complementary surfaces:

- a conventional PTY-backed ANSI/VT terminal grid for shells and ordinary
  command-line programs;
- a local graphics scene for applications that opt into the **Chymeral Link
  Protocol (CLP)**.

Programs remain ordinary binaries or launchers. They keep `stdin`, `stdout`,
`stderr`, arguments, and exit codes. A CLP-aware program additionally connects
to the private endpoint inherited from the Chymeral terminal session; it does
not become a host-loaded plugin.

## What it is not

- Not a mod system or an extension loader.
- Not a general widget toolkit.
- Not a replacement for a program's application logic, physics, or input
  policy.
- Not a global graphics service. Each Chymeral window owns its own local CLP
  endpoint and sessions.

## Current capabilities

- PTY terminal with ANSI/VT rendering, configurable font and visual
  appearance, image or animated GIF background, and optional startup splash.
- Retained 2D scene graph with roots, children, clipping, deterministic
  stacking, solid fills, and uploaded RGBA8 textures.
- Hit testing, hover, focus, drag-and-drop, and rectangular overlap detection.
- CLP v1.3 over Unix-domain sockets on Linux and named pipes on Windows, with
  per-window random tokens, capability negotiation, session ownership, and
  bounded resource use.
- Program delivery for events, resize, tick, frame, lifecycle, closing, and
  overload recovery notifications.
- A separate reference client library: the
  [Chymeral Rust SDK](https://github.com/Chymeral/chymeral-rust-sdk).

## Quick start

Build and launch from a Rust toolchain:

```text
cargo run --release
```

On Linux, Chymeral launches `CHYMERAL_SHELL`, then `$SHELL`, then `/bin/sh`.
On Windows, it launches `CHYMERAL_SHELL`, then `COMSPEC`, then `cmd.exe`.
Normal terminal programs work without modification. A program launched *inside*
that terminal can opt into CLP through an SDK or its own conforming client.

Chymeral loads an optional `chymeral.toml` from `CHYMERAL_CONFIG`, or from:

- Linux: `$XDG_CONFIG_HOME/chymeral/chymeral.toml`, falling back to
  `~/.config/chymeral/chymeral.toml`.
- Windows: `%APPDATA%\chymeral\chymeral.toml`.

Start with [`chymeral.toml.example`](chymeral.toml.example). Paths in that file
may be relative to the configuration file.

## Documentation

- [Main Manual](Main_Manual.md) — installation, configuration, terminal
  behavior, CLP concepts, event model, scene model, and testing guidance.
- [Technical Journal](Technical_Journal.md) — the architectural record and the
  completed M1–M14 milestones formerly kept in this README.
- [Chymeral Rust SDK](https://github.com/Chymeral/chymeral-rust-sdk) — the
  separate reference Rust client and its conformance examples.

## Project state

The implementation through M14 is complete: the terminal, local runtime, CLP
v1.3, overlap support, reference Rust SDK, conformance client, and Linux/Windows
validation are in place. Future work can add SDKs for other languages and a
language-neutral IDL without changing Chymeral into a plugin host.

## License

Chymeral is available under the [MIT License](LICENSE).
