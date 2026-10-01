# Chymeral Main Manual

This manual describes the current public behavior of Chymeral and CLP v1.3.
It is written for terminal users, application authors, and SDK implementers.
For the design and milestone history, see the [Technical Journal](Technical_Journal.md).

## 1. Product model

Chymeral is a minimalist terminal emulator with a reusable graphics runtime
local to each window. Its terminal grid and graphics scene are separate
surfaces that share the same runtime:

```text
Chymeral window
├── ANSI/VT terminal grid
└── retained 2D scene
    ├── underlay roots: behind terminal text
    └── overlay roots: in front of terminal text
```

The terminal remains useful by itself. A regular shell command does not need to
know that CLP exists. A compatible application is still a normal executable or
launcher, but it can use the graphics scene through CLP while retaining standard
streams and normal process semantics.

Chymeral is not a widget toolkit, mod manager, plugin loader, game engine, or
global daemon. It does not choose an application's language or load application
code into its own process.

## 2. Supported platforms and build

Chymeral targets Linux and Windows. Windows requires Windows 10 version 1809 or
newer, or Windows 11, because the terminal uses ConPTY. macOS is intentionally
not supported.

Build and run it from this repository with a current Rust toolchain:

```text
cargo run --release
```

The executable is `chymeral` on Linux and `chymeral.exe` on Windows. The normal
development checks are:

```text
cargo fmt --check
cargo test --all-targets -- --test-threads=1
cargo clippy --all-targets -- -D warnings
cargo build --release
```

## 3. Terminal behavior

On Linux, Chymeral starts the executable named by `CHYMERAL_SHELL`; otherwise it
uses `$SHELL`, then `/bin/sh`. On Windows it uses `CHYMERAL_SHELL`, then
`COMSPEC`, then `cmd.exe`. When that shell exits, Chymeral closes its window.

The terminal provides a PTY-backed ANSI/VT grid with UTF-8 input, common
control and navigation keys, terminal resizing, OSC 0/2 titles, alternate
screen support, bounded scrollback, and ANSI SGR colors. It sets
`TERM=xterm-256color`.

This is deliberately a focused terminal implementation, not a promise of every
VT extension. Features such as selection, visual scrollback navigation, mouse
terminal protocols, device-query coverage beyond its required startup behavior,
background color attributes, and text decorations are not a public compatibility
guarantee at this stage.

## 4. Configuration

Chymeral optionally reads `chymeral.toml`. The lookup order is:

1. The path in `CHYMERAL_CONFIG`.
2. Linux: `$XDG_CONFIG_HOME/chymeral/chymeral.toml`, or
   `~/.config/chymeral/chymeral.toml` when `XDG_CONFIG_HOME` is unset.
3. Windows: `%APPDATA%\chymeral\chymeral.toml`.

An absent file uses the built-in defaults. Unknown TOML keys are rejected so a
misspelled configuration cannot silently change behavior. Every relative path is
resolved from the directory containing `chymeral.toml`.

### 4.1 Complete example

[`chymeral.toml.example`](chymeral.toml.example) is the maintained template.
The following is a compact, valid configuration:

```toml
[terminal.font]
source = "system"             # "system" or "file"
family = "monospace"          # used by source = "system"
size = 16.0                    # logical pixels
line_height = 20.0             # logical pixels
cell_width = "auto"            # "auto" or a positive number

[terminal.foreground]
color = "#E5E7EB"

[terminal.background]
kind = "image"                # "color" or "image"
path = "images/background.gif" # required for kind = "image"
opacity = 0.72                 # 0.0 through 1.0
fit = "cover"                 # "cover", "contain", or "stretch"

[appearance.startup_splash]
enabled = true
image = "images/splash.png"
duration_ms = 1500
opacity = 1.0

[input.shortcuts]
diagnostics_events = "Ctrl+Shift+E"
diagnostics_ticks = "Ctrl+Shift+T"
diagnostics_frames = "Ctrl+Shift+F"

[testing_tools.scene]
enabled = false
```

For a file-backed font, set `source = "file"`, set `path`, and optionally set
`face_index` for a font collection. Font measurements use logical pixels and
scale with display DPI. A proportional font is accepted, but its glyph advances
may not visually match the terminal's fixed logical cells.

Foreground colors use `#RRGGBB`. ANSI applications retain their own colors;
their default foreground restores to `terminal.foreground.color`. Text and the
cursor are opaque.

Backgrounds accept a `#RRGGBB` color or a supported image. PNG, JPEG, and GIF
backgrounds are supported; GIF frames animate. Image opacity applies only to
the background. A transparent background requests a transparent window surface;
if the active compositor cannot provide one, Chymeral continues with an opaque
surface.

The startup splash is an optional centered static PNG, JPG, or JPEG image.
It disappears after `duration_ms`; GIF and other animated assets are not valid
for the splash.

### 4.2 Diagnostics and built-in scene fixtures

The three configurable diagnostic shortcuts open modal event, tick, and frame
views. `Escape` returns to the terminal, and keys typed while a diagnostic view
is open are not sent to the shell. Their defaults are `Ctrl+Shift+E`,
`Ctrl+Shift+T`, and `Ctrl+Shift+F`; the shortcuts must be distinct and include a
modifier.

`[testing_tools.scene]` is an internal visual fixture, disabled by default. It
can create colored test nodes before a CLP application is attached. Each node
has `id`, optional `parent`, `x`, `y`, `width`, `height`, `color`, `opacity`,
`clip_children`, `z_index`, and an optional root-only `layer` (`underlay` or
`overlay`). It is a test aid, not part of the CLP application API.

## 5. CLP sessions

**CLP** means *Chymeral Link Protocol*. It is a versioned, language-neutral
contract between a Chymeral window and an application process. The active
version is **CLP v1.3**.

When Chymeral starts its shell, the shell and its descendants inherit two
private variables:

```text
CHYMERAL_CLP_ENDPOINT
CHYMERAL_CLP_TOKEN
```

They identify an ephemeral endpoint for that one window: a Unix-domain socket
on Linux or a named pipe on Windows. The token is random per window. Do not put
these values in configuration, logs, command lines, or persistent application
state. They prevent accidental local connections but are not a security sandbox.

Applications launched outside Chymeral do not receive these variables. They may
choose to offer a terminal-only mode or report that Chymeral is required;
Chymeral does not provide a global service for them.

### 5.1 Lifecycle

The client begins with a version, the inherited token, and its requested
capabilities. A successful session follows this order:

```text
application → hello
Chymeral    → init
application → init_complete
Chymeral    → start
application → started
             running
```

`init` contains the negotiated protocol version, session identity, granted
capabilities, and launch context. A client can close cleanly, report an
application fault, or receive host shutdown. Every terminal path has a final
close reason. Closing one CLP session removes only that session's scene and
resources; it does not close the terminal window or shell.

The Rust SDK performs this handshake in `Client::connect_from_env`.

### 5.2 Capabilities and subscriptions

The client requests only the capabilities it needs. Chymeral grants their
intersection with its supported set:

| Capability | Enables |
| --- | --- |
| `scene` | Creating and changing retained nodes. |
| `resources` | Uploading and destroying session-owned RGBA8 textures. |
| `input` | Resize, focused keyboard input, raw input subscriptions, terminal state subscriptions, and scene interaction. |
| `timing` | Tick and frame subscriptions. |
| `overlap` | Rectangle overlap state, queries, and transitions; also requires `scene` for commands. |

After the lifecycle handshake, a client may replace its subscription set:

| Subscription | Requirement | Receives |
| --- | --- | --- |
| `raw_input` | `input` | Raw pointer movement, press, release, and scroll for the focused program session. |
| `terminal` | `input` | Terminal output-available and terminal-exited state. |
| `timing` | `timing` | Tick and frame notifications. |

Resize is delivered to every active session with `input`, first at activation
and again when the window's effective size or scale changes. Scene events do
not require an additional subscription. They are routed by node ownership.

### 5.3 Focus and multiple windows

The operating system window manager decides which Chymeral window receives
physical input. Only that focused window reacts to keyboard and pointer input.
Within it, a primary click on a program-owned scene node focuses that node and
sends subsequent keyboard input to its owning session. Clicking empty space
returns keyboard input to the terminal.

Raw pointer and scroll input go to the focused program session. Scene events
are delivered only when Chymeral can express their node handles for a session;
an application never learns another session's handles. A focus loss cancels an
active scene drag.

## 6. Scene and resources

CLP creates a retained tree of roots and children in logical pixels. A child
uses local coordinates relative to its parent. Parents paint before their
children. `z_index` orders siblings, and insertion order breaks equal values
deterministically.

A root belongs either to `underlay`, painted behind terminal text, or
`overlay`, painted in front of it. Root defaults to `overlay`. Nodes have these
properties:

```text
x, y, width, height
visible
clip_children
z_index
overlap_enabled
```

Node content is `empty`, a premultiplied RGBA8 `solid` color, or an `image`
that references an uploaded texture with independent opacity. The public scene
operations are:

```text
create_root       create_child       update_node
set_content       reparent           remove_node
upload_texture    destroy_texture    query_overlaps
```

`upload_texture` accepts exactly `width × height × 4` RGBA8 bytes. Chymeral
does not read arbitrary client file paths or accept base64 texture data. Handles
returned for nodes and textures are opaque, session-owned values. A session may
not access another session's handles; its nodes and textures are safely cleaned
up when it closes, faults, or disconnects.

The host validates dimensions, ownership, capabilities, texture usage, and
resource limits before applying a command. An individual texture is limited to
64 MiB of RGBA8 data and a session to 128 MiB, in addition to GPU limits.

## 7. Input, interaction, and overlap

Input is delivered as canonical events rather than exposing `winit` or `wgpu`
types. Keyboard events carry key identity, optional text, press/release state,
modifiers, and repeat state. Raw pointer events carry logical coordinates,
buttons, scroll delta, and scroll unit. Focus and terminal-state events are
also available.

The scene interaction layer produces these directed events:

```text
PointerEnter      PointerLeave       PointerMove
PointerPress      PointerRelease
FocusGained       FocusLost
DragStart         DragEnter          DragLeave
DragMove          Drop                DragEnd           DragCancel
OverlapEnter      OverlapLeave
```

Drag starts after a primary-button movement of at least four logical pixels.
The application owns the response: it may move the dragged node, reject a drop,
or ignore the event.

Overlap is intentionally geometric rather than physical. Nodes with
`overlap_enabled = true` use their world-space rectangles. Pointer location,
visibility, content, clipping, and `z_index` do not affect overlap; invisible
nodes can therefore serve as colliders. Touching only an edge is not an
overlap. There is no cross-session collision state.

`OverlapEnter` and `OverlapLeave` occur on a state transition. A program can
also call `query_overlaps` immediately after changing a node to retrieve its
current same-session contacts in deterministic order. The queried node itself
does not need to have overlap enabled. This supports keyboard-driven games and
layout rules without imposing physics. Image alpha silhouettes, collision
response, non-rectangular shapes, native gamepads, multitouch, and external
drag-and-drop are not part of CLP v1.3.

## 8. Timing and delivery pressure

With the `timing` capability and subscription, an application receives fixed
ticks and rendered frames. A frame may follow zero or more ticks; it is not a
fixed-FPS promise. Durations are supplied to the client so application logic can
remain time-aware.

CLP protects the window from a slow or stalled client. Each running session has
a bounded notification queue. Continuous data — ticks, frames, pointer moves,
drag moves, and terminal output-available state — may be coalesced so the most
recent useful sample is retained. Before normal delivery resumes, Chymeral
sends `DeliveryGap` with the affected categories. The client must rebuild any
derived state that depended on those continuous samples.

Discrete events and resize notifications are never silently discarded. If a
session cannot make room even after continuous data is displaced, Chymeral ends
only that session with `host_overloaded`; the terminal and other sessions stay
alive. Transient operating-system backpressure is retried through the running
session writer instead of being treated as an immediate protocol failure.

## 9. Rust client quick start

The reference Rust SDK is developed and released separately as
[`chymeral-rust-sdk`](https://github.com/Chymeral/chymeral-rust-sdk). It is a
client library, not a host plugin and not a requirement for other languages.

For local development, add a path dependency to an application project:

```toml
[dependencies]
chymeral-rust-sdk = { path = "../chymeral-rust-sdk" }
```

Run the application from a Chymeral terminal, then connect and create a node:

```rust
use chymeral_rust_sdk::{
    Capabilities, Capability, Client, NodeContent, NodeProperties, SceneLayer,
};

fn main() -> chymeral_rust_sdk::Result<()> {
    let mut client = Client::connect_from_env(Capabilities::from([
        Capability::Scene,
    ]))?;

    let _node = client.create_root(
        SceneLayer::Overlay,
        NodeProperties::new(48.0, 48.0, 160.0, 96.0),
        NodeContent::Solid { color: [249, 115, 22, 255] },
    )?;

    let _reason = client.close()?;
    Ok(())
}
```

The SDK exposes typed capabilities, session and opaque-handle types, node
properties and patches, scene and texture commands, subscriptions,
notifications, close reasons, and client errors. Its `clp_conformance` example
checks lifecycle, subscriptions, timing, resources, scene mutation, overlap,
and clean closure. Its `clp_probe` example is the interactive orange/blue drag
test.

CLP itself remains language-neutral. Other language SDKs or direct protocol
clients have equal architectural status when they conform to the negotiated
protocol and session rules.

## 10. Operational testing

Use the configuration fixture only for visual internal checks. For CLP behavior,
start Chymeral, open its terminal, then run the Rust SDK examples from the SDK
repository:

```text
cargo run --example clp_conformance
cargo run --example clp_probe
```

The conformance client must return successfully to the same terminal. The probe
shows orange and blue nodes: drag orange onto blue, observe overlap transitions,
then press `q` while they overlap. Only the probe session closes; the Chymeral
terminal remains open.

## 11. Boundaries and next steps

CLP v1.3 deliberately favors a small, reliable foundation. It has no public
package manifest standard, no protocol IDL, no public SDKs beyond the Rust
reference, no widget library, and no 3D scene API. Future language bindings can
share an IDL and the conformance suite without changing the ordinary-program
model at the center of Chymeral.
