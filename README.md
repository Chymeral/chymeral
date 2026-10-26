<p align="center">
  <img src="branding/chymeral_logo.svg" alt="Chymeral logo" width="192">
</p>

<p align="center">
  <a href="https://github.com/Chymeral/chymeral/actions/workflows/ci.yml"><img src="https://github.com/Chymeral/chymeral/actions/workflows/ci.yml/badge.svg" alt="Chymeral quality checks"></a>
  <a href="https://github.com/Chymeral/chymeral/releases/latest"><img src="https://img.shields.io/github/v/release/Chymeral/chymeral?display_name=tag&amp;sort=semver" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/Chymeral/chymeral" alt="MIT License"></a>
</p>

# Chymeral

> A minimalist terminal emulator with a reusable local graphics runtime for
> applications compatible with CLP.

**Chymeral** keeps the normal terminal
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
  appearance, and image or animated GIF background.
- Retained 2D scene graph with roots, children, clipping, deterministic
  stacking, rectangles, unified RGBA8/GIF images, and intrinsic text nodes.
- Alpha-aware image hit testing, hover, focus, drag-and-drop, passthrough
  overlays, and opt-in scene overlap detection.
- CLP v2.4 over Unix-domain sockets on Linux and named pipes on Windows, with
  per-window random tokens, capability negotiation, session ownership, bounded
  image and text resource use, and reliable atomic scene batches.
- Program delivery for events, resize, tick, frame, lifecycle, closing, and
  overload recovery notifications.
- Separate official client libraries: the
  [Chymeral Rust SDK](https://github.com/Chymeral/chymeral-rust-sdk), the
  reference and conformance client, and the
  [Chymeral Node SDK](https://github.com/Chymeral/chymeral-node-sdk) for
  Node.js and TypeScript applications.

## Current release: Chymeral 1.7.0

Chymeral `1.7.0` implements **CLP `v2.4`**. Its accumulated runtime includes
bounded frame-paced `presentation`, session-owned `window_metadata` for native
titles and static icons, and reliable atomic `scene_batch` revisions alongside
the acknowledged incremental `set_node` control plane. A scene batch applies a
bounded create/update/remove revision as one retained-scene state and requests
one redraw; it does not promise display scanout timing. The host accepts
compatible earlier clients but grants only the capabilities they negotiate. See
the v2.4 sections in the [Main Manual](Main_Manual.md) and the
[v2.4 fixture corpus](protocol/fixtures/v2.4/README.md).

This release keeps configuration intentionally narrow: terminal
appearance, explicit scene-text font aliases, and diagnostic shortcuts. It is
not an application-scene or startup-scripting format; the former testing-scene
and startup-splash sections have been retired without compatibility aliases.

## Downloads

The quickest way to use Chymeral is a portable edition from the
[latest release](https://github.com/Chymeral/chymeral/releases/latest). No
installation is required:

- **Linux x86_64:** extract the archive and run `Launch Chymeral.sh`. The
  launcher explicitly selects X11; a Wayland desktop therefore needs Xwayland
  and `DISPLAY` available.
- **Windows x86_64:** extract the archive and run `Launch Chymeral.cmd`.

Each archive includes its launcher, configuration file, application icon,
license, and documentation. See the [Main Manual](Main_Manual.md) for
configuration and platform behavior.

## Build from source

Building from source requires Rust `1.90` or newer. The repository follows the
current Rust `stable` channel for local development and verifies `1.90`
separately in CI. Build and launch with:

```text
cargo run --locked --release
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
- [Latest release](https://github.com/Chymeral/chymeral/releases/latest) —
  portable Linux and Windows editions.
- [Technical Journal](Technical_Journal.md) — the architectural record and the
  milestone record formerly kept in this README.
- [Chymeral Rust SDK](https://github.com/Chymeral/chymeral-rust-sdk) — the
  separate reference Rust client and its conformance examples, published on
  [crates.io](https://crates.io/crates/chymeral-rust-sdk).
- [Chymeral Node SDK](https://github.com/Chymeral/chymeral-node-sdk) — the
  separate Node.js and TypeScript client, published on
  [npm](https://www.npmjs.com/package/@chymeral/node-sdk).
- [CLP v2.4 fixture corpus](protocol/fixtures/v2.4/README.md) — versioned
  conformance vectors for the current complete CLP contract.
- [Contributing guide](CONTRIBUTING.md) — setup, checks, and protocol-change
  expectations for collaborators.
- [Changelog](CHANGELOG.md) — released behavior and compatibility notes.
- [Security policy](SECURITY.md) — private vulnerability-reporting guidance.

## Project state

Chymeral `1.7.0` is the current stable release on `main`. The independently
versioned official Rust and Node SDK releases are also at `1.7.0` and implement
CLP v2.4. The application is distributed as a terminal program, CLP is the
language-neutral contract, and each SDK is one client implementation. Linux
and Windows manual acceptance has passed, including native-Wayland title
behavior and its documented dynamic-icon limitation. Portable editions are
available from
[GitHub Releases](https://github.com/Chymeral/chymeral/releases/latest).

## License

Chymeral is available under the [MIT License](LICENSE).
