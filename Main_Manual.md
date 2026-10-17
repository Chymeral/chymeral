# Chymeral Main Manual

This manual describes the public CLP v2.0 contract. It is written for terminal
users, application authors, and SDK implementers.
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
screen support, bounded scrollback, pointer selection, system-clipboard copy
and paste, and ANSI SGR colors. It sets `TERM=xterm-256color`.

Use the pointer wheel over unobscured terminal space to view scrollback. Drag
with the primary pointer button to select primary-screen text, including
scrollback, then copy it with `Ctrl+Shift+C`. `Ctrl+Shift+V` inserts regular
system-clipboard text into the PTY without an implicit Enter. For safety,
multiline text is accepted only while the foreground terminal program has
enabled bracketed paste (DECSET 2004); Chymeral then sends the standard
bracketed-paste delimiters. Linux primary selection and middle-click paste are
not implemented.

This is deliberately a focused terminal implementation, not a promise of every
VT extension. Features such as terminal mouse reporting protocols, device-query
coverage beyond its required startup behavior, background color attributes, and
text decorations are not a public compatibility guarantee at this stage.

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

# Optional file-backed aliases for CLP intrinsic scene text:
# [scene_text.fonts.ui]
# path = "fonts/ui.otf"
# face_index = 0

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

`[scene_text.fonts]` maps a client-visible alias to an explicit local font file.
It is optional: scene text without a `font` style uses the terminal font. An
alias must be non-empty and unpadded; a client cannot supply a font path or ask
the host to resolve an installed system-family name. This keeps custom scene
text layout reproducible when the same configuration assets are supplied.

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
contract between a Chymeral window and an application process. The public
contract is **CLP v2.0**. It intentionally does not accept CLP v1.x clients or
provide command aliases for their prior vocabulary.

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
| `images` | Uploading and removing session-owned RGBA8 or GIF images. |
| `input` | Resize, focused keyboard input, raw input subscriptions, terminal state subscriptions, and scene interaction. |
| `timing` | Tick and frame subscriptions. |
| `overlap` | Rectangle overlap state, queries, and transitions; also requires `scene` for commands. |
| `external_drop` | Incoming desktop-file drops targeted to opt-in scene nodes. |
| `text` | Creating and changing intrinsic scene-text nodes; also requires `scene`. |

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
active scene drag. Program focus persists when its focused node is hidden or
fully clipped, so a program can restore its own scene from keyboard input; a
primary click on empty terminal space or removal of the focused node returns
that focus to the terminal.

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
external_drop_enabled
```

Nodes use one of four kinds: `empty`, `rectangle`, `image`, or `text`.
`rectangle` carries an RGBA8 color. `image` references one unified static RGBA8
or GIF image handle and has independent opacity. Text is a separate intrinsic
family because the host computes its width and height. The public scene
operations are:

```text
create_node       set_node           remove_node
upload_image      remove_image
is_overlapping    get_overlaps
```

Each session owns one implicit retained scene tree. `create_node` creates a
root when `parent` is omitted, or a child when it names an owned node. `layer`
is optional for roots only and defaults to `overlay`; it must be omitted for
children. A successful result contains `node-N`, plus `text_metrics` for a text
node. There is no `create_scene_tree` operation because a session cannot create
another scene.

`set_node` applies one validated patch atomically. Its `changes` object may
include `properties`, `parent`, `layer`, `visual`, `text`, or `text_style`.
Omit a field to preserve it. A parent change uses either
`{ "kind": "root" }` or `{ "kind": "node", "node": "node-N" }`.
`layer` remains root-only. Visual nodes may switch among `empty`, `rectangle`,
and `image`; text nodes may change text or style but cannot become visual, and
visual nodes cannot become text. Create a replacement node for a cross-family
change.

`upload_image` sends metadata followed by consecutive binary CLP frames. Its
source is either `{ "kind": "rgba8", "width": W, "height": H }`, requiring
exactly `W × H × 4` RGBA8 bytes, or `{ "kind": "gif", "byte_length": N }`,
requiring exactly `N` encoded GIF bytes. Chymeral does not read client paths or
accept base64 image data. It returns one opaque, session-owned `image-N` handle
for either source. `remove_image` rejects an image that a node still references.
Nodes and images are safely cleaned up when a session closes, faults, or
disconnects.

The host validates dimensions, ownership, capabilities, image use, and resource
limits before applying a command. An individual static image is limited to
64 MiB of RGBA8 data; the combined static-image and decoded-GIF total is
limited to 128 MiB per session, in addition to GPU limits.

Command rejection uses a stable machine-readable `code` plus a diagnostic
message. The v2 codes are `unsupported_version`, `capability_denied`,
`invalid_command`, `invalid_handle`, `not_owner`, `image_in_use`,
`resource_limit`, `invalid_argument`, `invalid_node_change`, `host_capacity`,
`host_unavailable`, and `protocol_violation`. Clients should branch only on the
code; messages are for people and diagnostics.

### 6.1 Intrinsic scene text

With `scene` and `text`, an application creates a `text` node through
`create_node`. Its origin is the upper-left corner of the first cell, not the
visible ink bounds. The host returns terminal-cell metrics and uses the
resulting rectangle for drawing, clipping, hit testing, dragging, and opt-in
overlap or external-drop targeting.

Text accepts printable Unicode, explicit `\n`, and `\t`. Tabs advance to local
eight-cell stops; wide and combining Unicode follow terminal cell-width rules.
There is no automatic wrapping, ellipsis, rich-text spans, or text-owned clip
box. The widest explicit line determines width, and explicit rows determine
height. Empty text is `0 × 0`; a trailing newline adds a blank row. Text may be
start-, center-, or end-aligned inside the widest line's intrinsic width.

An omitted style inherits terminal font, size, line height, and foreground
color. A style may choose a configuration-owned `scene_text.fonts` alias, size,
line height, RGBA8 color, normal/bold weight, and alignment. Updating text or
style recalculates its intrinsic rectangle; generic width or height patches and
generic content replacement are rejected for text nodes.

Chymeral bounds an individual text payload to 1 MiB and 256 Ki terminal cells.
A session retains at most 16 MiB of text, 1 Mi terminal cells, and 1,024 text
nodes. These limits prevent a client from exhausting host memory or rendering
resources.

### 6.2 Animated scene images

With `images`, a client uploads encoded GIF bytes through the same
`upload_image` operation used for RGBA8 images. It declares
`{ "kind": "gif", "byte_length": N }`, sends exactly `N` consecutive binary
bytes, receives `image-N`, then creates or updates an `image` node using that
handle.

An animated resource uses the same explicit rectangle, clipping, stacking,
hit-testing, drag, overlap, and external-drop behavior as a static image node.
It does not infer a rectangle from GIF dimensions and no client filesystem path
is accepted. `remove_image` rejects an image that is still referenced by an
owned node; removing those nodes or closing the session makes it releasable.

Chymeral decodes and schedules GIF frames itself. Delays are clamped to 20 ms
through 10 s. A delayed host turn advances at most one frame, so it never emits
a catch-up burst. A GIF with no loop extension plays once and retains its final
frame; an explicit zero loop value is infinite, while other explicit values
retain their GIF repetition semantics. Every node sharing a resource displays
the same phase. If all of those nodes are invisible or fully clipped, playback
pauses with its remaining delay intact and resumes from that phase when a node
becomes visible.

The encoded upload is limited to 16 MiB, an animation to 300 frames and 64 MiB
of decoded RGBA8 data, and every frame must meet the active GPU dimension and
64 MiB image limits. GIF is the only animated-source format. APNG, animated
WebP, video, and property interpolation are not part of this contract.

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
call `is_overlapping` for the direct boolean state or `get_overlaps` for its
current same-session contacts in deterministic order. The queried node itself
does not need to have overlap enabled. This supports keyboard-driven games and
layout rules without imposing physics. Image alpha silhouettes, collision
response, non-rectangular shapes, native gamepads, and multitouch are outside
the current contract. Incoming external file drop uses the `external_drop`
capability; outgoing desktop drag-and-drop is not implemented.

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
    Capabilities, Capability, Client, NodeInput, NodeProperties, SceneLayer,
};

fn main() -> chymeral_rust_sdk::Result<()> {
    let mut client = Client::connect_from_env(Capabilities::from([
        Capability::Scene,
    ]))?;

    let _node = client.create_node(
        None,
        Some(SceneLayer::Overlay),
        NodeInput::Rectangle {
            properties: NodeProperties::new(48.0, 48.0, 160.0, 96.0),
            color: [249, 115, 22, 255],
        },
    )?;

    let _reason = client.close()?;
    Ok(())
}
```

The `0.2.0` development SDK exposes typed CLP v2.0 capabilities, session and
opaque-handle types, nodes, atomic node changes, unified images, subscriptions,
notifications, close reasons, and client errors. `clp_conformance` exercises
lifecycle, timing, images, text, scene mutation, and overlap against a live
Chymeral endpoint, including both RGBA8 and GIF image uploads.

CLP itself remains language-neutral. Other language SDKs or direct protocol
clients have equal architectural status when they conform to the negotiated
protocol and session rules.

## 10. Operational testing

Use the configuration fixture only for visual internal checks. For CLP behavior,
start Chymeral, open its terminal, then run the Rust SDK examples from the SDK
repository:

```text
cargo run --example clp_conformance
```

The conformance client must return successfully to the same terminal without
closing the surrounding Chymeral terminal.

## 11. Boundaries and next steps

CLP deliberately favors a small, reliable foundation. It has no public package
manifest standard, no protocol IDL, no widget library, and no 3D scene API.
The Rust reference SDK is not a privileged host component; future language
bindings can share an IDL and the conformance suite without changing the
ordinary-program model at the center of Chymeral.
