# Changelog

All notable user-visible changes to Chymeral are recorded here. This project
uses [Semantic Versioning](https://semver.org/) for the terminal application;
the CLP protocol and language SDKs have independent version numbers.

## [Unreleased]

### Added

- CLP v2.4 `scene_batch`, a negotiated reliable command for a bounded atomic
  retained-scene construction or structural revision. It supports ordered
  create, update, and removal operations with batch-local aliases.
- A 512-operation, 512 KiB JSON-envelope, and 128-byte alias contract;
  rejected batches restore the retained scene, node ownership, allocator, and
  overlap-delivery state.
- A versioned CLP v2.4 fixture corpus with byte-checked atomic scene-batch
  lifecycle, alias, result, and closure coverage for independent SDKs.

### Changed

- Successful scene batches reconcile only their final overlap state and request
  one redraw. They do not imply compositor acknowledgement or physical scanout.
- Configuration now contains only terminal appearance, explicit scene-text font
  aliases, and diagnostic controls. The internal `[testing_tools.scene]` and
  `[appearance.startup_splash]` sections have been removed; diagnostic
  shortcuts now live under `[diagnostics.shortcuts]`.

### Fixed

- Queued CLP host frames are emitted in bounded transport writes, so a
  scene-batch response larger than the Windows named-pipe buffer reaches its
  client instead of remaining pending indefinitely.

### Compatibility

- The unreleased Chymeral `1.7.0` development line implements CLP v2.4. It
  continues to accept v2.0 and v2.1 clients without granting later negotiated
  capabilities.
- The retired `[testing_tools.scene]`, `[appearance.startup_splash]`, and
  `[input.shortcuts]` configuration sections are deliberately rejected; no
  compatibility aliases or sequential fallback are provided.

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
