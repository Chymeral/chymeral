# Changelog

All notable user-visible changes to Chymeral are recorded here. This project
uses [Semantic Versioning](https://semver.org/) for the terminal application;
the CLP protocol and language SDKs have independent version numbers.

## [Unreleased]

No unreleased user-visible changes.

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
