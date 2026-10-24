# Chymeral Main Manual

This manual describes CLP v2.4 in the Chymeral `1.7.0` development line. A
v2.4 host continues to accept v2.0 and v2.1 clients, which retain their
existing behavior without later capabilities. It is written for terminal
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

## 2. Supported platforms and installation

Chymeral targets Linux and Windows. Windows requires Windows 10 version 1809 or
newer, or Windows 11, because the terminal uses ConPTY. macOS is intentionally
not supported.

### 2.1 Portable editions

Portable Linux x86_64 and Windows x86_64 editions are available from the
[latest GitHub release](https://github.com/Chymeral/chymeral/releases/latest).
They require no installation: extract the matching archive and run its launcher.
Each archive includes a bundled `chymeral.toml`, application icon, license, and
documentation.

On Linux, run `Launch Chymeral.sh`. The portable launcher explicitly selects
the X11 backend for predictable desktop integration and external-drop behavior.
When it is launched from a Wayland desktop, an Xwayland server and `DISPLAY`
must be available. On Windows, run `Launch Chymeral.cmd`.

### 2.2 Build from source

Building from source requires Rust `1.90` or newer. The repository follows the
current Rust `stable` channel for development and verifies `1.90` separately
in CI. Build and run it with:

```text
cargo run --locked --release
```

The executable is `chymeral` on Linux and `chymeral.exe` on Windows. The normal
development checks are:

```text
cargo fmt --check
cargo test --locked --all-targets -- --test-threads=1
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release --all-targets
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

On Linux, normal launches let `winit` select the active desktop backend. Setting
`CHYMERAL_FORCE_X11=1` explicitly selects X11 for Chymeral's window. In a
Wayland session this requires an available Xwayland server and `DISPLAY`; it is
useful for portable launches that need the X11 external-drop path. This setting
does not remove or rewrite the Wayland session variables inherited by terminal
children.

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
contract between a Chymeral window and an application process. The released
contract is **CLP v2.4**, including presentation, application-window-metadata,
and atomic-scene-batch extensions. A v2.4 host accepts v2.0 and v2.1 clients
without granting later capabilities. No supported version accepts CLP v1.x
clients or provides command aliases for their prior vocabulary.

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
| `overlap` | Opt-in scene contact state, queries, and transitions; also requires `scene` for commands. |
| `external_drop` | Incoming desktop-file drops targeted to opt-in scene nodes. |
| `text` | Creating and changing intrinsic scene-text nodes; also requires `scene`. |
| `presentation` | CLP v2.1 bounded, coalesced geometry packets committed at a renderer frame; also requires `scene`. |
| `window_metadata` | CLP v2.1 session-owned native title and bounded static RGBA8 icon claim. |
| `scene_batch` | CLP v2.4 reliable, bounded atomic scene construction or structural revision; also requires `scene`. |

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
input_passthrough
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
present_scene (CLP v2.1)
set_window_metadata (CLP v2.1)
scene_batch (CLP v2.4)
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

`image` contact defaults to the complete node rectangle. An image may instead
select alpha contact with `{ "kind": "alpha", "threshold": N }` in its
`contact_shape`, where `N` is an inclusive value from `1` through `255`.
`{ "kind": "bounds" }` is the default. Alpha contact samples uploaded RGBA8
source alpha or the current decoded GIF frame; alpha zero does not contact.
The chosen shape applies consistently to pointer targeting, drag/drop targets,
incoming external-file-drop targets, direct overlap queries, and overlap
transitions.

`input_passthrough` defaults to `false` for every node family. When true, a
node still paints but is skipped for pointer-derived and external-drop target
selection. Its children remain independently targetable, and the property does
not alter visibility, paint order, focus already owned by the program, or
overlap participation. It is intended for decorative or layered content above
an interactive node.

`upload_image` sends metadata followed by consecutive binary CLP frames. Its
source is either `{ "kind": "rgba8", "width": W, "height": H }`, requiring
exactly `W × H × 4` RGBA8 bytes, or `{ "kind": "gif", "byte_length": N }`,
requiring exactly `N` encoded GIF bytes. Chymeral does not read client paths or
accept base64 image data. It returns one opaque, session-owned `image-N` handle
for either source. `remove_image` rejects an image that a node or its session's
window-icon metadata still references. Nodes and images are safely cleaned up
when a session closes, faults, or disconnects.

The host validates dimensions, ownership, capabilities, image use, and resource
limits before applying a command. An individual static image is limited to
64 MiB of RGBA8 data; the combined static-image and decoded-GIF total is
limited to 128 MiB per session, in addition to GPU limits.

### 6.1 Atomic scene batches (CLP v2.4)

`scene_batch` is the reliable structural path for a complete first scene or a
coherent retained-scene revision. Request both `scene` and `scene_batch`, then
send one ordinary command envelope containing ordered `create_node`,
`set_node`, and `remove_node` operations:

```json
{
  "type": "command",
  "request_id": "initial-panel",
  "command": {
    "op": "scene_batch",
    "operations": [
      {
        "op": "create_node",
        "alias": "panel",
        "layer": "overlay",
        "node": { "kind": "rectangle", "properties": { "x": 8, "y": 12, "width": 240, "height": 120 }, "color": [36, 88, 216, 255] }
      },
      {
        "op": "create_node",
        "alias": "label",
        "parent": { "alias": "panel" },
        "node": { "kind": "text", "properties": { "x": 16, "y": 24 }, "text": "Ready", "style": {} }
      },
      {
        "op": "set_node",
        "node": { "alias": "panel" },
        "changes": { "properties": { "clip_children": true } }
      }
    ]
  }
}
```

Every `create_node` declares a unique UTF-8 alias; aliases contain no control
characters and are limited to 128 bytes. A later operation may refer to that
alias as `{ "alias": "name" }`, while an existing node uses its ordinary
`"node-N"` handle. Aliases must be declared earlier in the same batch, so
parents naturally precede their children. A child must omit `layer`; the usual
root-layer and hierarchy rules remain unchanged.

A batch contains one through 512 operations and its complete JSON command
envelope is limited to 512 KiB. It may use image, text, overlap, or external
drop properties only when the corresponding already-negotiated capability is
also granted. Image upload/removal, window metadata, subscriptions, and
`present_scene` remain separate operations.

Chymeral validates the full revision, restores the prior retained scene and
node-handle allocation if any operation fails, and exposes no intermediate
tree. It reconciles overlap transitions only against the final scene. A
successful batch requests one redraw, but neither the command result nor its
response promises a compositor or physical-display scanout. The ordered result
contains one outcome per operation and a `nodes` map for aliases that remain
live after commit; a temporary alias created and removed in the same batch is
therefore absent from that map.

The capability deliberately has no automatic sequential fallback. If
`scene_batch` is absent, an SDK returns a capability error and the application
must explicitly choose its own incremental `create_node`/`set_node` path. This
avoids silently turning a requested atomic first paint into visible progressive
assembly.

### 6.2 Frame-paced presentation (CLP v2.1)

`present_scene` is a separate, opt-in visual lane for continuous state such as
dragging a viewport, scrolling a scene subtree, or moving a related control.
It is **not** a replacement for `set_node`: structural work, resources,
content, hierarchy, visibility, interaction flags, and styles stay on the
ordered command path.

The client must negotiate both `scene` and `presentation`, then sends a JSON
message rather than a `command` envelope:

```json
{
  "type": "present_scene",
  "presentation_id": "drag-42",
  "updates": [
    { "node": "node-1", "properties": { "x": 128, "y": 72 } },
    { "node": "node-9", "properties": { "x": 128 } }
  ]
}
```

Only `x`, `y`, `width`, and `height` are eligible. An intrinsic text node may
change only `x` and `y`, because Chymeral continues to calculate its rectangle
from its text and style. A packet has one through 256 distinct owned node
handles; its identifier is non-empty UTF-8 of at most 128 bytes; and its JSON
payload is at most 64 KiB. Every handle, ownership check, node kind, and
geometry value is validated before any change applies, so a rejected packet
does not partially alter the retained scene.

Chymeral keeps one newest unpresented packet per session. A newer packet
replaces stale visual state before the next renderer opportunity, and a later
reliable scene mutation replaces still-unpresented visual state for that
session. The selected packet applies atomically immediately before rendering.
This intentionally favors the newest pointer or scroll position over a queue
of historic positions.

The host later sends a `presentation_status` notification:

```json
{
  "type": "presentation_status",
  "session_id": "session-1",
  "presentation_id": "drag-42",
  "status": "presented"
}
```

The possible statuses are `presented`, `superseded`, and `rejected`; a rejected
status also has `code` and `message`. Statuses are advisory and coalesced under
pressure, not a reliable per-packet acknowledgement queue. `presented` means
that Chymeral reached its host `queue.present` call, not that a compositor or
display has reported physical scanout. Applications should submit the newest
transient state without awaiting every status, while retaining `set_node` for
work that must be ordered and acknowledged.

Set `CHYMERAL_PRESENTATION_TRACE=1` before launching a development build to
write host-stage diagnostics. The trace labels its contextual host
input-to-transport, transport-to-commit, commit-to-submit, and
transport-to-`present` timings. It explicitly labels application delivery and
physical scanout as unavailable because neither has a portable host timestamp.

Command rejection uses a stable machine-readable `code` plus a diagnostic
message. The v2 codes are `unsupported_version`, `capability_denied`,
`invalid_command`, `invalid_handle`, `not_owner`, `image_in_use`,
`resource_limit`, `invalid_argument`, `invalid_node_change`,
`invalid_scene_batch`, `host_capacity`, `host_unavailable`, and
`protocol_violation`. Clients should branch only on the code; messages are for
people and diagnostics.

### 6.3 Application window metadata (CLP v2.1)

With `window_metadata`, a session may supply the title and icon identifying its
ordinary program inside the native Chymeral window. This is intentionally not
desktop-window management: it cannot move, resize, decorate, persist, or open
another window. A client uses the ordered command lane:

```json
{
  "op": "set_window_metadata",
  "title": "Example editor",
  "icon": "image-3"
}
```

Both fields are an independent patch. Omit a field to preserve it; write JSON
`null` to clear it. At least one field must be present. A nonempty successful
update refreshes that session's claim recency; clearing both fields removes its
claim. A title is presented verbatim without a `Chymeral —` prefix. It must be
nonempty, have no Unicode control character, and occupy at most 256 UTF-8
bytes.

`icon` reuses an image handle previously uploaded by the same session. It must
be static RGBA8, at most 256 × 256 pixels and 256 KiB. GIF handles, paths,
URLs, base64 payloads, foreign handles, and images above those limits are
rejected. A selected icon remains referenced until it is cleared or its session
ends, so `remove_image` rejects it meanwhile.

Every session retains its own claim. The newest live nonempty claim is visible;
clearing or closing that session restores the newest remaining claim. When no
claim remains, Chymeral restores its terminal-derived title and bundled icon.
Terminal OSC title updates do not override an active application title. Host
diagnostics temporarily use `Chymeral — Diagnostics`, then resume this same
arbitration rule when they close.

The `init.context.window_metadata.icon_supported` value and the successful
command result both report whether the active backend can apply a dynamic icon:

```json
{ "kind": "window_metadata", "icon_supported": true }
```

Windows and supported Linux backends return `true`. Native Wayland can apply
the title but reports `false` for dynamic icons; it keeps the bundled Chymeral
icon instead. Chymeral does not force X11 for this feature. CLP v2.0 clients
cannot request `window_metadata` and retain ordinary terminal title/icon
behavior.

### 6.4 Intrinsic scene text

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

### 6.5 Animated scene images

With `images`, a client uploads encoded GIF bytes through the same
`upload_image` operation used for RGBA8 images. It declares
`{ "kind": "gif", "byte_length": N }`, sends exactly `N` consecutive binary
bytes, receives `image-N`, then creates or updates an `image` node using that
handle.

An animated resource uses the same explicit rectangle, clipping, stacking,
hit-testing, drag, overlap, and external-drop behavior as a static image node.
When it uses alpha contact, Chymeral refreshes its interactive silhouette from
the current decoded GIF frame and reconciles affected overlap transitions.
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
`overlap_enabled = true` use their world-space rectangles by default; an image
that explicitly selects alpha contact contributes only its source-alpha
silhouette. Pointer location, visibility, clipping, and `z_index` do not alter
overlap participation, so invisible nodes can serve as colliders. Touching
only an edge is not an overlap. There is no cross-session collision state.

`OverlapEnter` and `OverlapLeave` occur on a state transition. A program can
call `is_overlapping` for the direct boolean state or `get_overlaps` for its
current same-session contacts in deterministic order. The queried node itself
does not need to have overlap enabled. This supports keyboard-driven games and
layout rules without imposing physics. Alpha image silhouettes are supported;
collision response, arbitrary vector shapes, native gamepads, and multitouch
remain outside the current contract. Incoming external file drop uses the
`external_drop` capability; outgoing desktop drag-and-drop is not implemented.

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

In CLP v2.1, `presentation_status` is intentionally an advisory continuous
notification. It can coalesce to the latest host outcome and is not included
in `DeliveryGap`; do not use it as a required per-packet completion queue. Use
it for diagnostics and keep the newest presentational state locally.

## 9. Official SDK quick starts

Chymeral currently has two stable external client SDKs:

- [`chymeral-rust-sdk`](https://github.com/Chymeral/chymeral-rust-sdk), the
  reference and conformance client for Rust, published on
  [crates.io](https://crates.io/crates/chymeral-rust-sdk).
- [`@chymeral/node-sdk`](https://github.com/Chymeral/chymeral-node-sdk), the
  Node.js and TypeScript client, published on
  [npm](https://www.npmjs.com/package/@chymeral/node-sdk).

They are client libraries, not host plugins and not requirements for other
languages.

### Shared fixture corpus

The versioned [CLP v2.0 fixture corpus](protocol/fixtures/v2.0/README.md)
provides portable ordered examples of framing, lifecycle, capabilities, scene
commands, binary image transfer, notifications, errors, and closure. It is the
shared regression reference for language SDKs; it does not make an SDK a host
dependency and it does not replace a live endpoint conformance run. SDK authors
should pin a reviewed Chymeral revision when copying those vectors into their
own test harnesses rather than downloading them during ordinary CI. The Rust
and Node SDKs vendor a byte-checked snapshot with `SHA256SUMS` and replay it
through their public APIs and own fake hosts in their Linux/Windows CI matrices.

The versioned [CLP v2.1 fixture corpus](protocol/fixtures/v2.1/README.md)
extends that method for `presentation` and `window_metadata`. It includes an
accepted frame packet, a rejected intrinsic-text bounds update, and title/icon
patch preservation and clearing.

The versioned [CLP v2.4 fixture corpus](protocol/fixtures/v2.4/README.md)
adds an atomic `scene_batch` construction vector. It verifies local aliases,
parent-before-child ordering, the ordered result, and a clean close through the
same framed transport.

### Rust

For a released version, add the crate from `crates.io` to an application
project:

```toml
[dependencies]
chymeral-rust-sdk = "1.5"
```

For local development, add a path dependency to an application project:

```toml
[dependencies]
chymeral-rust-sdk = { path = "../chymeral-rust-sdk" }
```

The published `1.5.x` crate implements CLP v2.1. Until the `1.7.0` release is
published, use the `current-dev` SDK checkout through the path dependency to
exercise CLP v2.4 scene batches.

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

The `1.7.0` development SDK line exposes typed CLP v2.4 capabilities, session
and opaque-handle types, nodes, `SceneBatch`, atomic node changes, unified
images, subscriptions, notifications, close reasons, client errors, alpha
image contact, generic node input passthrough, frame-paced presentation, and
application window metadata. `clp_conformance` exercises lifecycle, timing,
images, text, scene mutation, batches, and overlap against a live Chymeral
endpoint, including both RGBA8 and GIF image uploads.

### Node.js and TypeScript

The Node SDK requires Node.js `22` or newer and uses ESM. Install it in an
application project with:

```text
npm install @chymeral/node-sdk
```

Run the application from a Chymeral terminal so it inherits the private CLP
endpoint and token. The [Node SDK README](https://github.com/Chymeral/chymeral-node-sdk#readme)
contains its TypeScript quick start, API reference, and live conformance
instructions.

CLP itself remains language-neutral. The Rust and Node SDKs have equal
architectural status with direct protocol clients and future language SDKs when
they conform to the negotiated protocol and session rules.

## 10. Operational testing

Use the configuration fixture only for visual internal checks. For CLP behavior,
start Chymeral, open its terminal, then run a conformance client from an SDK
repository. For Rust:

```text
cargo run --locked --example clp_conformance
```

For the CLP v2.1 presentation path, launch Chymeral with
`CHYMERAL_PRESENTATION_TRACE=1`, then run the Node probe from its SDK checkout:

```text
npm run example:presentation
```

Click the blue panel once to focus the probe, move the pointer, scroll, resize
the window, then press `b` for its 180-update 60 Hz burst and `q` to print the
observed application-submit-to-host-status summary. Compare the probe output
with the host trace; neither is physical input-to-photon timing.

For CLP v2.4 scene batches, run either SDK conformance client from a Chymeral
terminal. Each client creates, updates, and removes retained nodes through an
atomic batch, checks the returned aliases, and then removes its remaining
resources before closing only its own session. The host and both SDK test suites
also replay the exact v2.4 fixture corpus. For a visible first-paint check from
the Node SDK checkout, run:

```text
npm run example:scene-batch
```

It creates a complete panel of nested rectangles and text through one batch.
Confirm it appears as one coherent composition rather than visible serial
assembly, then click it to focus the session and press `q`. This closes only
the probe session, leaving the terminal open.

For CLP v2.1 application window metadata, run the Node SDK probe from a
Chymeral terminal:

```text
npm run example:window-metadata
```

Click its blue panel to focus the session. Its initial static icon and verbatim
title should replace Chymeral's fallback on Windows and supported Linux
backends. Press `t` to replace only the title while preserving the icon, `c` to
clear both fields and restore the terminal-derived title plus bundled icon,
`r` to restore the claim, then `q` to close only that CLP session. Native
Wayland must still update the title while the probe reports dynamic icon support
as unavailable.

For Node.js and TypeScript:

```text
npm ci
npm run example:conformance
```

Each conformance client must return successfully to the same terminal without
closing the surrounding Chymeral terminal.

To validate alpha image contact and node passthrough interactively, run the
Rust client with `cargo run --locked --example clp_conformance -- --manual` or
the Node client with `npm run example:conformance -- --manual`. The manual
scenario shows a blue target, an orange alpha-frame image, a pink GIF tile that
alternates between opaque and transparent, and a visible white band with
`input_passthrough`. The terminal identifies each directed scene event and the
GIF's overlap transitions: the orange rim must target the image; its transparent
center and the white band must target the blue node; the pink tile must target
only while visible. Where `external_drop` is granted, dropping a desktop file
on those regions follows the same routing. Click a scene node and press `q` to
close only the client session.

## 11. Boundaries and next steps

CLP deliberately favors a small, reliable foundation. It has no public package
manifest standard, no protocol IDL, no widget library, and no 3D scene API.
Neither the Rust reference SDK nor the Node SDK is a privileged host component;
additional language bindings can share the versioned fixture corpus and the
conformance suite without changing the ordinary-program model at the center of
Chymeral. An IDL remains deferred until it removes real duplication that the
fixtures and independent SDKs cannot address.

The current stable source baseline on `main` is Chymeral `1.5.0`, which
implements CLP v2.1. The `current-dev` branch prepares Chymeral and the two
supported SDKs at `1.7.0` over CLP v2.4; their package versions do not version
the protocol or the terminal application. The host Rust crate exists to share
implementation between the executable and its tests. It is not a supported
application-development API: programs should use CLP directly or a language
SDK.
