# Changelog

All notable user-visible changes to Chymeral are recorded here. This project
uses [Semantic Versioning](https://semver.org/) for the terminal application;
the CLP protocol and language SDKs have independent version numbers.

## [Unreleased]

### Fixed

- Windows `CHYMERAL_SHELL` values ending in `.cmd` or `.bat` now start through
  `COMSPEC /d /s /c`, so conventional application launchers can serve as the
  direct PTY shell instead of failing under `CreateProcessW`.

## [1.5.0] - 2026-10-22

### Added

- A CLP v2.1 `presentation` capability for bounded, atomic
  frame-paced geometry packets. It keeps the newest unpresented packet per
  session while preserving the ordered `set_node` control plane.
- Advisory `presentation_status` notifications (`presented`, `superseded`, or
  `rejected`) and `CHYMERAL_PRESENTATION_TRACE=1` host-stage diagnostics.
- A versioned CLP v2.1 presentation fixture corpus with exact byte checks for
  SDK conformance work.
- A CLP v2.1 `window_metadata` capability with the ordered
  `set_window_metadata` command for session-owned native titles and bounded
  static RGBA8 icons.
- Deterministic newest-claim arbitration, title/icon restoration after clearing
  or closure, backend icon-support feedback, and byte-checked v2.1 metadata
  fixture coverage.

### Changed

- Image-scene rendering now retains vertex chunks and updates only chunks whose
  visible geometry changed, while retaining uploaded image resources.
- Terminal scrollback rows now retain their historical width safely after a
  window resize, including their rendering, selection, and copy paths.

### Compatibility

- Chymeral `1.5.0` implements CLP v2.1. The earlier `1.0.x` portable editions
  implement CLP v2.0.
- A CLP v2.1 host continues to accept CLP v2.0 clients without granting the
  new `presentation` or `window_metadata` capabilities.

## [1.0.0] - 2026-10-17

### Added

- A PTY-backed ANSI/VT terminal with configurable typography, visual effects,
  static or animated image backgrounds, and clipboard selection/paste.
- A retained local 2D scene runtime with rectangles, static RGBA8 images,
  animated GIF images, intrinsic text, hierarchy, clipping, deterministic
  stacking, focus, drag/drop, and overlap queries.
- CLP v2.0 over local Unix-domain sockets on Linux and named pipes on Windows,
  with per-session capability negotiation, bounded resources, ownership, and
  lifecycle/input/timing delivery.
- Alpha-derived image contact, generic input passthrough, terminal and scene
  external-file-drop routing where the platform backend supports it.

### Compatibility

- Chymeral `1.0.x` implements CLP v2.0 only. CLP v1.x and provisional v1.4
  clients are deliberately not accepted.
- The reference `chymeral-rust-sdk` `1.0.x` is the tested Rust binding for this
  contract. Other conforming language clients have equal protocol status.

### Requirements

- Building the core from source requires Rust `1.90` or newer.
