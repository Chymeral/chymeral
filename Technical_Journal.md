# Chymeral Technical Journal

This document preserves the technical record that previously lived in the
project README. It is a development journal and architectural reference, not
the product introduction. Use the [Main Manual](Main_Manual.md) for current
operation and public behavior.

## Product decisions retained

Chymeral is a minimalist terminal emulator plus a reusable local graphics
runtime. The terminal grid and 2D scene are different surfaces in one window.
This distinction prevents the graphics runtime from replacing normal terminal
semantics or from becoming a generic widget framework.

The consumer of graphics capabilities is always an ordinary program: a native
binary or conventional launcher that retains standard streams and process
behavior. CLP is language-neutral. Rust, Python, Lua, JavaScript, C#, Go,
C/C++, Zig, Java, and other viable clients have the same architectural status.
SDKs are client libraries; Chymeral never loads them as plugins. The earlier
`MLExtension` concept is therefore not a central component.

Linux and Windows are the supported platforms. macOS is explicitly deferred
rather than implicitly unsupported. The renderer uses `wgpu` and `glyphon`; the
terminal uses a PTY and a dedicated ANSI/VT state and grid.

## Milestone record

| Milestone | Delivered foundation | Decisions established |
| --- | --- | --- |
| M1 | PTY terminal, ANSI/VT grid, `wgpu`/`glyphon` rendering, visual configuration, image/GIF background, and splash. | The terminal remains independently useful; graphical appearance is configuration, not an application API. |
| M2 | Canonical `EngineEvent`; `Init`, `Start`, `Tick`, and `Frame` runtime lifecycle. | Runtime phases and frame timing are host-owned, and application-facing delivery must preserve that model. |
| M3 | Retained scene graph. | Parent-before-child painting, deterministic sibling ordering, and distinct terminal/scene surfaces. |
| M4 | Texture ownership and rendering resources. | Resources use opaque handles and must have explicit lifetime validation. |
| M5 | Scene images and image rendering. | Images share scene geometry and composition rather than creating a parallel UI subsystem. |
| M6 | Hit testing. | Hit testing is distinct from collision/overlap, even where they share geometry. |
| M7 | Hover and focus. | A scene node can route keyboard ownership without changing terminal process semantics. |
| M8 | Drag-and-drop. | Drag produces events; Chymeral does not prescribe application movement or drop policy. |
| M9 | Clipping, layers, and visual test tools. | Underlay and overlay roots make terminal/scene composition explicit. |
| M10 | Internal program contract and capabilities. | Program sessions are explicit, versioned, and isolated. |
| M11 | Local CLP transport, token handshake, and session cleanup. | Each window creates a private Unix socket or Windows named pipe; there is no global service. |
| M12 | Scene commands, RGBA8 uploads, opaque handles, ownership, and resource limits. | The transport handler does not mutate UI or GPU state directly; validated commands reach the window thread through bounded work. |
| M13 | Application delivery for event, tick, frame, resize, closing, and close reason. | Continuous notifications may be coalesced, while discrete input and resize are protected from silent loss. |
| M13.2 | Optional rectangular overlap queries and transitions. | Overlap is geometric, same-session, and deliberately not a physics engine. |
| M14 | Standalone Rust SDK, conformance client, visual probe, cross-platform CI, and public SDK repository. | The first SDK is a reference client, not a Rust privilege or a host dependency. |
| M15 | Terminal pointer selection, scrollback viewport, system clipboard, and bracketed paste. | Terminal cells remain the selection model; selection is primary-screen only, and multiline paste is safe by default. |

## CLP evolution

CLP began as an internal lifecycle contract and became an actual local transport
without exposing host memory, Rust types, graphics backend types, or a global
socket. The active protocol is **v1.3**. It has five opt-in capabilities:
`scene`, `resources`, `input`, `timing`, and `overlap`.

The negotiated lifecycle is deliberately strict:

```text
hello → init → init_complete → start → started → running
```

Node and texture identifiers are opaque and session-scoped. Command validation
checks ownership, capability, geometry, resource use, and lifecycle before any
window-state change. On closure, fault, or disconnection, the owning session's
nodes and textures are removed without affecting other sessions.

M13 hardened delivery under pressure. The bounded host-to-client queue retains
the newest meaningful continuous sample and reports `DeliveryGap` when a client
must reconstruct derived state. It does not silently drop discrete input or
resize; a session that cannot preserve them is isolated with `host_overloaded`.
The running-session writer also treats short-lived operating-system backpressure
as retryable work, avoiding premature session loss on Windows named pipes.

## Scene and interaction policy

Scene roots paint in `underlay` or `overlay`; children paint on top of parents.
`z_index` sorts siblings and insertion order resolves ties. The graphics scene
has logical-pixel coordinates and shares the window's scale factor with the
terminal, but neither surface is reduced to the other.

Hit testing, focus, hover, drag, and overlap are separate concepts:

- Hit testing chooses the visual target under a pointer.
- Focus identifies the program session that receives keyboard input in the
  focused Chymeral window.
- Drag describes pointer-derived source and target transitions.
- Overlap compares opt-in world-space rectangles regardless of pointer,
  visibility, clipping, paint order, or image content.

That policy gives applications composable primitives rather than prebuilt game
physics or controls. A keyboard-driven game can move a node, query overlap, and
restore its last valid position. A desktop application can use the same
primitive for placement rules. Neither requires mouse input.

## Validation record

The M1–M15 implementation has been exercised on Linux and Windows. The core
validation suite covers formatting, unit and integration tests, warnings-denied
Clippy, and release builds. The Rust SDK has a separate GitHub Actions matrix
for Rust 1.85 and stable on both platforms, including formatting, tests,
Clippy, and release builds of its examples.

The reference conformance program validates lifecycle, timing subscriptions,
RGBA8 upload/destruction, scene mutation, same-session overlap, and clean
session closure. The visual probe validates drag routing and overlap transitions
while proving that closing a CLP program does not close its terminal.

M15 adds a cell-based selection over primary-screen scrollback and visible
terminal output, preserving wide characters, combining marks, and soft-wrap
boundaries. `Ctrl+Shift+C` and `Ctrl+Shift+V` use the regular system clipboard.
Pasting never inserts Enter; NUL text is rejected, and multiline input requires
DECSET 2004 so Chymeral can send standard bracketed-paste delimiters. Linux and
Windows manual checks confirmed selection, clipboard transfer, scrollback,
single-line paste, both bracketed-paste states, and alternate-screen behavior.

## Intentional boundaries

The following are deferred, not accidental omissions:

- SDKs for languages other than Rust and a shared protocol IDL.
- Package manifests, application discovery, or a central program registry.
- A widget toolkit, host-loaded extensions, physics, or native gamepad policy.
- Alpha-silhouette or arbitrary-shape collision, multitouch, and external
  drag-and-drop.
- 3D scene authoring. `wgpu` is the renderer backend, not a commitment to turn
  Chymeral into a general-purpose 3D engine.

These boundaries keep the project aligned with its central proposition: an
ultra-austere terminal that can host rich local 2D application surfaces without
forcing users to assemble a separate UI toolchain.
