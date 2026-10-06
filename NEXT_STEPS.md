# Chymeral Next Steps

This is the working execution record for the milestones that follow M14. It is
not the public product manual and it is not a historical record yet. Once a
milestone is accepted, its confirmed technical decisions and validation evidence
must be summarized into [Technical_Journal.md](Technical_Journal.md).

## Current baseline

- M1–M17 are complete.
- CLP v1.3 is the current public protocol contract.
- The Rust SDK is the reference client implementation for CLP v1.3.
- No phase below has started unless its status explicitly says otherwise.

## How to maintain this record

For each phase:

1. Keep its scope narrow; do not silently include adjacent work.
2. Record a decision only after it is accepted, using the decision template.
3. Record the exact validation command, platform, and manual test evidence
   before marking the phase complete.
4. Move the durable result into `Technical_Journal.md` and the user-facing
   behavior into `Main_Manual.md` after acceptance.
5. Preserve compatibility notes whenever CLP, configuration, or SDK behavior
   changes.

### Decision template

```text
Date:
Decision:
Context:
Rationale:
Consequences:
Validation:
```

## Protocol position for M15–M19

M15 changes terminal behavior only and does not require a CLP revision. M16,
M17, and M18 are the planned scene/protocol expansion. Their public shapes must
remain provisional until they are frozen together as **CLP v1.4** in M19.

CLP v1.3 clients must remain supported. New behavior must be behind negotiated
capabilities rather than inferred from a host version alone. The precise v1.4
capability names are not final until M19, but the current planning names are
`external_drop`, `text`, and `animated_images`.

## M15 — Terminal Interaction

**Status:** Complete

### Scope

- Pointer-based terminal text selection.
- Scrollback navigation sufficient to select previous terminal output.
- Copying selected text to the system clipboard.
- Pasting system-clipboard text into the PTY.
- Bracketed paste when the terminal application has enabled it.
- Safe, predictable handling of multiline paste.
- Initial conventional shortcuts: `Ctrl+Shift+C` and `Ctrl+Shift+V`.

### Exclusions

- Scene-node text and CLP text commands.
- External file drops.
- Terminal mouse reporting protocols.
- A terminal widget framework.

### Acceptance criteria

- Selection and copy preserve UTF-8 terminal text across multiple lines.
- Selection remains correct for wide and combining Unicode characters.
- Pasting never adds an implicit Enter key.
- Bracketed-paste-aware programs receive the correct delimiters.
- Core tests cover selection and paste semantics without requiring a desktop.
- Manual tests pass on Linux and Windows using a real system clipboard.

### Accepted decisions

- **Date:** 2026-10-01
  **Decision:** Selection uses primary-screen logical cell coordinates with an
  exclusive end point. It may span scrollback and the visible grid; it is not
  available while the alternate screen is active.
  **Context:** The terminal grid stores wide characters across a leading and a
  continuation cell, while combining marks belong to the prior glyph.
  **Rationale:** Cell geometry keeps hit testing, highlighting, and copied
  UTF-8 deterministic without adding a second text-layout model. Selecting any
  portion of a wide glyph selects the complete glyph. Soft-wrapped rows do not
  add a copied newline; explicit terminal line boundaries do.
  **Consequences:** Terminal selection is intentionally independent of scene
  nodes. A visible scene node keeps pointer precedence at its location.
  **Validation:** Unit tests cover scrollback, soft wrapping, wide Unicode, and
  stable history viewing.

- **Date:** 2026-10-01
  **Decision:** Use the operating-system text clipboard through `arboard`, with
  `Ctrl+Shift+C` and `Ctrl+Shift+V` as the initial shortcuts.
  **Context:** Chymeral supports Linux and Windows, including both X11 and
  Wayland desktop sessions.
  **Rationale:** The library provides a native clipboard abstraction without
  exposing platform-specific clipboard logic through CLP.
  **Consequences:** Linux primary selection and middle-click paste are deferred;
  copy and paste use the regular system clipboard only.
  **Validation:** Compile and lint coverage is platform-neutral; real clipboard
  transfer still requires Linux and Windows desktop checks.

- **Date:** 2026-10-01
  **Decision:** Plain clipboard text never implies Enter. NUL-containing text
  is rejected. Multiline text is rejected unless the foreground terminal
  application enabled DECSET 2004; when enabled, the exact text is wrapped in
  `CSI 200~` and `CSI 201~`.
  **Context:** Accidental multiline paste can execute commands in a shell.
  **Rationale:** Rejection is safe and predictable without adding a confirmation
  dialog to an ultra-austere terminal. Bracketed paste delegates correct
  multiline handling to applications that opted in.
  **Consequences:** Users must enable bracketed paste in a compatible shell or
  editor before pasting multiple lines.
  **Validation:** Unit tests cover payload generation, NUL rejection, and the
  multiline guard.

- **Date:** 2026-10-01
  **Decision:** Scrolling the pointer wheel over terminal space moves the
  scrollback viewport. A wheel event over a visible scene node remains available
  to the focused CLP application.
  **Context:** The terminal grid and scene graph share a window but remain
  separate surfaces.
  **Rationale:** This makes terminal history usable without stealing scrolling
  from an interactive graphics surface.
  **Consequences:** Keyboard scrollback shortcuts are deferred until a real
  compatibility need justifies reserving more terminal input.
  **Validation:** Unit tests cover viewport stability while new output arrives;
  manual pointer-wheel checks remain required.

### Manual desktop validation

1. Launch `target/release/chymeral`, print enough distinct UTF-8 lines to fill
   more than one screen, then use the wheel to reach history and return to the
   bottom.
2. Drag-select text spanning visible output and scrollback, including a wide
   character and a combining mark. Copy with `Ctrl+Shift+C`, paste into a native
   editor, and compare the exact UTF-8 text and intended line boundaries.
3. Copy one line from a native editor, focus the Chymeral terminal, and press
   `Ctrl+Shift+V`. Confirm the text arrives without an implicit execution; press
   Enter manually only after observing it.
4. Disable bracketed paste with `printf '\e[?2004l'`, copy multiple lines, and
   confirm that `Ctrl+Shift+V` rejects them without sending input. Check the
   Chymeral process log for the explanatory message.
5. Enable it with `printf '\e[?2004h'`, run `cat -v`, paste the same multiple
   lines, and verify `^[[200~` before and `^[[201~` after the exact payload.
   Stop `cat` with `Ctrl+C`, then disable the mode with `printf '\e[?2004l'`.
6. Enter an alternate-screen program such as `less` or `vim`, verify that
   pointer selection does not act there, leave the program, and confirm primary
   screen selection still works.

### Validation record

- Linux build host: `cargo fmt --check`, `cargo test --all-targets --
  --test-threads=1`, `cargo clippy --all-targets -- -D warnings`,
  `cargo build --release`, and `git diff --check` pass on 2026-10-01
  (154 tests).
- Manual Linux and Windows: pointer selection, scrollback navigation, and
  copying the selected text to the regular system clipboard pass on 2026-10-01.
- Manual Linux and Windows: a single-line clipboard paste reaches the PTY but
  does not execute until the user explicitly presses Enter on 2026-10-01.
- Manual Linux and Windows: multiline paste rejects while DECSET 2004 is
  disabled, then passes with the standard delimiters while it is enabled, as
  accepted on 2026-10-01.
- Manual Linux and Windows: selection is unavailable in an alternate-screen
  application such as `nano`, while applications that remain in the primary
  screen such as the observed `top` session retain selection, as accepted on
  2026-10-01.

## M16 — Incoming External Drag and Drop

**Status:** Implementation complete; Linux X11/XWayland and Windows
scene-target validation passed; final desktop regression matrix pending

### Scope

- Receive files dropped from the desktop or file manager.
- Insert dropped file paths into the terminal without executing them.
- Route a drop to an opt-in CLP scene node when one is the eligible target.
- Deliver enter, over, leave, and drop transitions to that node's owning
  session.
- Use terminal insertion as the fallback when no eligible scene target exists.

### Exclusions

- Generating a drag operation from Chymeral to another application.
- Moving, deleting, opening, or reading dropped files on the host's behalf.
- Text, HTML, and arbitrary binary external payloads.
- Cross-session scene-handle disclosure.

### Pending technical decisions

- The future configuration form for selecting `powershell` or a custom-shell
  terminal quoting policy.
- Final public names remain subject to the CLP v1.4 freeze in M19.

### Acceptance criteria

- A dropped file is inserted as text only; no command is executed implicitly.
- POSIX and `cmd` quoting produce usable paths containing spaces and quotes.
- Only the owning opt-in session receives a scene drop event.
- Dropping over no eligible node falls back to terminal insertion.
- Core tests cover routing, ownership, quoting, and malformed data.
- Manual file-drop tests pass on Windows, X11, and Wayland where supported.

### Accepted decisions

- **Date:** 2026-10-03
  **Decision:** CLP represents every external file as a canonical percent-
  encoded `file:` URI.
  **Context:** Linux filenames are byte sequences that need not be UTF-8,
  while CLP messages are text-based and must remain language-neutral.
  **Rationale:** An ASCII URI preserves the original POSIX path bytes through
  percent encoding and represents Windows Unicode paths without exposing host
  path types through CLP.
  **Consequences:** CLP clients receive URIs rather than host-native path
  strings and explicitly decode them for their platform.
  **Validation:** Unit and cross-platform manual tests must include spaces,
  quotes, Unicode, and non-UTF-8 POSIX filenames where the desktop permits it.

- **Date:** 2026-10-03
  **Decision:** Chymeral never inserts a dropped path containing a line break
  or other terminal control character into the PTY. An eligible CLP scene node
  may still receive its URI.
  **Context:** A POSIX filename can contain characters that would become
  terminal input boundaries.
  **Rationale:** Dropping a file must never execute a command or inject terminal
  control sequences.
  **Consequences:** Terminal fallback can reject a valid but unsafe filename;
  application clients retain the option to handle it explicitly.
  **Validation:** Unit tests must prove no PTY write occurs for unsafe terminal
  fallback paths.

- **Date:** 2026-10-03
  **Decision:** Terminal fallback quotes a batch of paths automatically: POSIX
  syntax on Linux and `cmd` syntax on Windows. A future configuration option
  may select `powershell` or a custom-shell policy.
  **Context:** Chymeral launches conventional shells but cannot infer every
  third-party shell's quoting grammar safely.
  **Rationale:** Platform defaults are useful without making arbitrary shell
  syntax a hidden host policy.
  **Consequences:** Paths with spaces and quotes are usable by default; custom
  shell integration remains explicit configuration work.
  **Validation:** Unit and manual tests must cover spaces, quotes, and several
  paths in one batch on both platforms.

- **Date:** 2026-10-03
  **Decision:** The topmost eligible opt-in scene node receives an external
  drop. Nodes without opt-in are transparent; when no eligible node exists,
  Chymeral falls back to terminal insertion.
  **Context:** The terminal grid and retained scene are separate surfaces that
  can overlap in one window.
  **Rationale:** This follows existing hit-test ordering without allowing a
  passive graphics node to block useful terminal behavior.
  **Consequences:** The receiving session learns only its own target handle.
  Multiple files form one logical drop batch.
  **Validation:** Routing and ownership tests must cover overlapping nodes,
  disabled nodes, and terminal fallback.

- **Date:** 2026-10-03
  **Decision:** An external drag hovering a Chymeral window does not change its
  keyboard focus. A successful drop targets that window but does not alter a
  focused CLP node's keyboard ownership.
  **Context:** External drag-and-drop is deliberate operating-system input, not
  ordinary pointer interaction.
  **Rationale:** A hover must not unexpectedly redirect keyboard input; a drop
  should still reach the window beneath it.
  **Consequences:** External-drop transitions are independent of M13 pointer
  focus routing.
  **Validation:** Manual multi-window tests must verify hover and drop behavior
  without unintended focus changes.

- **Date:** 2026-10-03
  **Decision:** The provisional CLP v1.4 capability is named `external_drop`.
  A scene node opts in with `external_drop_enabled`; its owner receives
  `external_drop_enter`, coalescible `external_drop_over`,
  `external_drop_leave`, and `external_drop` events.
  **Context:** External input must remain discoverable, rate-safe, and unable
  to reveal a foreign scene handle.
  **Rationale:** A separate negotiated capability makes the feature explicit.
  Its events carry only the owning session's opaque node handle and a file URI
  list on the final drop.
  **Consequences:** The development host advertises draft CLP v1.4 behavior;
  CLP v1.3 clients remain supported but cannot request `external_drop`. M19
  freezes or revises these provisional names before public SDK exposure.
  **Validation:** Unit tests cover negotiation, capability denial, scene
  ownership, target precedence, and event serialization.

- **Date:** 2026-10-04
  **Decision:** Retain stable `winit 0.30.13`; M16 incoming file drops support
  Windows and X11, while native Wayland reception is deferred until a stable
  upstream API exposes it.
  **Context:** The stable Wayland backend does not deliver file-drop events or
  expose the surface, connection, and event queue needed to attach a safe
  downstream `wl_data_device` handler.
  **Rationale:** A local `winit` fork or beta dependency would make a small
  host capability into a high-maintenance windowing dependency. Silently
  forcing XWayland would also degrade the user's selected backend.
  **Consequences:** Chymeral detects Wayland at startup, reports that M16 file
  drops require X11, and does not grant `external_drop` to CLP clients on that
  backend. Windows and X11 retain the same canonical M16 events and terminal
  fallback.
  **Validation:** The startup diagnostic and capability negotiation must
  compile on Linux and Windows; manual drop validation covers Windows and X11.
  Native Wayland reception is deferred with the implementation.

### Validation record

- Linux build host: `cargo fmt --check`, `cargo test --all-targets --
  --test-threads=1`, `cargo clippy --all-targets -- -D warnings`, `cargo build
  --release`, and `git diff --check` pass on 2026-10-04 (105 library tests,
  63 binary tests, and one inherited-process integration test).
- Automated M16 coverage verifies canonical URI encoding including non-UTF-8
  POSIX paths, safe POSIX and `cmd` multi-file quoting with Unicode and quotes,
  batch limits, whole-batch rejection for terminal control characters, target
  precedence, ownership, cancellation routing, and backend-dependent
  capability negotiation.
- Final manual regression matrix: on Linux X11/XWayland, drop a multi-file
  selection containing spaces, quotes, and Unicode; on Windows, use spaces,
  `&`, `%`, `!`, and Unicode because quotes are not valid filename characters.
  Exercise both the raw terminal and the SDK probe's blue node, cancel a hover
  over that node, then repeat with two Chymeral windows and verify that a drop
  reaches only the window under the operating-system gesture without changing
  keyboard focus. Native Wayland reception is explicitly deferred.
- Manual terminal fallback: a desktop-file drop inserts the quoted path without
  executing it on Linux through X11/XWayland and on Windows on 2026-10-04.
  The current Windows release was rebuilt from this checkout before the test.
- Manual Windows scene target: the rebuilt `chymeral-rust-sdk` `clp_probe`
  created its orange and blue nodes, and delivered a dropped desktop file to
  the opt-in blue node as a `file:` URI on 2026-10-04.
- Manual Linux X11/XWayland scene target: the same rebuilt SDK probe created
  its nodes and received the desktop-file URI on its opt-in blue node on
  2026-10-04. Native Wayland reception remains explicitly deferred.

## M17 — Intrinsic Scene Text

**Status:** Complete; Linux and Windows visual validation passed

### Scope

- Add scene text nodes with intrinsic bounds calculated from occupied text
  cells.
- Use terminal font, foreground color, cell width, and line height when a text
  style does not override them.
- Support one style per text node initially: configured font alias, size, line height,
  color, weight, alignment, and clipping as applicable.
- Recalculate bounds, hit testing, focus, drag, and overlap when text changes.
- Treat explicit line breaks as new cell rows.

### Locked design direction

- A text node remains a scene node, not a widget.
- Its position continues to use the scene's logical-pixel coordinates.
- Its width is the widest occupied line in cells multiplied by cell width.
- Its height is the occupied line count multiplied by line height.
- Normal characters, wide characters, and combining marks use the terminal's
  cell-width rules.
- Backgrounds, borders, and complex controls remain separate scene nodes.

### Exclusions

- Rich text spans with several styles inside one node.
- Automatic paragraph layout beyond explicit line breaks.
- A general layout or widget system.
- Text input controls, IME composition, and accessibility tree work.

### Acceptance criteria

- Intrinsic bounds are deterministic for ASCII, wide Unicode, combining marks,
  tabs, and explicit line breaks.
- Default text visually follows configured terminal appearance.
- Text updates reconcile scene interaction and overlap correctly.
- Parent clipping and layer ordering match existing scene-node behavior.
- Unit tests cover geometry; visual probes pass on Linux and Windows.

### Accepted decisions

- **Date:** 2026-10-05
  **Decision:** A text node's local origin is the upper-left corner of the
  first cell on its first line, rather than the visible ink bounds of its
  first glyph.
  **Context:** Glyph bearings, overhangs, ascenders, and descenders vary by
  font and would make an ink-based origin unsuitable for retained-scene
  geometry.
  **Rationale:** Cell geometry keeps the declared position, clipping, hit
  testing, dragging, and overlap deterministic. A leading space occupies the
  first cell without moving the node origin.
  **Consequences:** The renderer positions glyph ink within its cell; clients
  position text from its cell rectangle, not from individual glyph pixels.
  **Validation:** Geometry and visual tests must cover leading spaces,
  ascenders, descenders, and styled fonts.

- **Date:** 2026-10-05
  **Decision:** Empty text has zero intrinsic width and height. An explicit
  line break creates a subsequent row, including a final empty row after a
  trailing newline; empty rows contribute line height but no width.
  **Context:** Scene text must expose predictable bounds without invisible
  one-cell interaction targets.
  **Rationale:** This preserves the terminal meaning of explicit newlines
  while ensuring that text with no occupied cells cannot accidentally capture
  pointer or overlap interactions.
  **Consequences:** `"hello\\n"` is two rows high and as wide as `"hello"`;
  `"\\n"` is two rows high with zero width; `""` is `0 × 0`.
  **Validation:** Unit tests must cover empty text, leading and trailing
  newlines, and blank intermediate rows.

- **Date:** 2026-10-05
  **Decision:** Horizontal tabs advance to fixed stops every eight cells,
  measured from the local origin of each text line.
  **Context:** A tab is not a glyph with a fixed pixel width, and its geometry
  would otherwise vary by client expectation.
  **Rationale:** Eight-cell stops are conventional terminal behavior and make
  intrinsic widths independent of the surrounding terminal cursor position.
  **Consequences:** Each explicit line begins again at column zero for tab-stop
  calculation.
  **Validation:** Unit tests must cover tabs at column zero, between glyphs,
  after wide characters, and on subsequent lines.

- **Date:** 2026-10-05
  **Decision:** Text starts aligned by default; center and end alignment place
  shorter lines within the intrinsic width of the longest line. M17 has no
  wrapping, ellipsis, or text-owned clipping.
  **Context:** Intrinsic nodes have no client-supplied layout box, while
  existing parent clipping already defines retained-scene visibility.
  **Rationale:** This supports aligned multi-line labels without creating an
  implicit layout or widget system.
  **Consequences:** Text that extends beyond a parent is visible unless that
  parent enables existing child clipping.
  **Validation:** Visual and geometry tests must cover all alignments and
  clipping by an ancestor.

- **Date:** 2026-10-05
  **Decision:** The default text face is the terminal face. Optional custom
  faces use aliases declared by the Chymeral configuration and resolved to
  explicit font files; clients never provide host paths or rely on installed
  system-family fallback.
  **Context:** System font-family resolution differs across Linux and Windows,
  but CLP clients require reproducible scene layout.
  **Rationale:** Configuration-owned aliases retain local control while making
  the selected font explicit and portable when the same configuration assets
  are supplied.
  **Consequences:** Requesting an unknown alias is a command error. M17 will
  define bounded text retention; exact resource limits remain an implementation
  detail to validate against renderer behavior.
  **Validation:** Configuration, command-rejection, and cross-platform visual
  tests must cover defaults, valid aliases, and unknown aliases.

- **Date:** 2026-10-05
  **Decision:** The provisional CLP v1.4 capability is named `text`. It adds
  `create_text_root`, `create_text_child`, `set_text`, and `set_text_style`.
  Creation returns an opaque node handle plus host-computed metrics; text and
  style updates return new metrics. The Rust SDK exposes matching typed
  `TextNodeProperties`, `TextStyle`, `TextMetrics`, and `TextNode` values.
  **Context:** Text needs an intrinsic rectangle, so regular scene creation
  with client-supplied width and height cannot represent its contract.
  **Rationale:** Separate commands make intrinsic ownership explicit while
  preserving all existing retained-node hierarchy, layer, interaction, and
  capability rules.
  **Consequences:** Generic width/height patches and generic content changes
  reject text nodes. Text nodes may still move, toggle visibility, change
  z-index, opt into overlap, and opt into external drops when the corresponding
  capabilities are granted.
  **Validation:** Unit and transport tests cover command serialization,
  capability denial, intrinsic-bound updates, and overlap reconciliation; the
  reference SDK conformance example exercises create and update operations.

- **Date:** 2026-10-05
  **Decision:** Retained text is bounded to 1 MiB and 256 Ki cells per payload,
  and to 16 MiB, 1 Mi cells, and 1,024 text nodes per session.
  **Context:** Text is retained host-side and can cause CPU, memory, glyph-cache,
  and draw-work pressure even without uploaded textures.
  **Rationale:** Independent limits bound the most direct abuse paths without
  coupling text accounting to image-resource quotas.
  **Consequences:** Command rejection is deterministic before a node or text
  update is committed; disconnect cleanup releases all retained text with its
  owned scene nodes.
  **Validation:** Layout and resource-path tests cover payload limits and
  session accounting; full render stress remains part of desktop validation.

### Validation record

- Linux build host: `cargo fmt --check`, `cargo test --all-targets --
  --test-threads=1` (181 tests), `cargo clippy --all-targets -- -D warnings`,
  `cargo build --release`, and `git diff --check` pass on 2026-10-05.
- Automated host coverage currently includes cell measurement for ASCII, wide
  and combining Unicode, tabs, empty and trailing-newline rows, style scaling,
  command ownership, capability denial, intrinsic-bound updates, and overlap
  reconciliation.
- The separate Rust SDK passes `cargo fmt --check`, `cargo test --all-targets`,
  and `cargo clippy --all-targets -- -D warnings`; `clp_conformance` now
  exercises text creation plus text/style updates.
- `clp_text_probe` passed visual validation on Windows and Linux on 2026-10-05:
  its one panel plus two text children were visible in the scene diagnostic;
  default terminal styling, child offsets, tab stops, the wide `界` glyph,
  updates, bold/end styling, and session-only close behavior all worked.
- Linux validation also configured the explicit `host_hack` alias to its
  file-backed Hack Regular font. The probe rendered with that alias on creation
  and after a style update, while retaining the visible wide-glyph fallback.

## M18 — Animated Scene Images

**Status:** Complete

### Scope

- Add retained animated-image resources for scene nodes.
- Accept encoded GIF bytes from a CLP client; never accept a client path for
  the host to read.
- Decode, schedule, and redraw animation frames inside Chymeral.
- Allow an animated resource to be shared by several nodes without duplicating
  resource ownership.
- Keep static RGBA8 texture upload fully supported.

### Locked design direction

- This milestone is animated image content, not property interpolation or a
  general node-animation system.
- GIF is the first supported animated source format.
- The resource/API name must remain generic enough for future APNG or WebP
  support without redesigning node ownership.
- Chymeral owns playback scheduling; a client does not need `timing` merely to
  display an animated image.

### Exclusions

- Position, opacity, color, or size tweening.
- Client-provided filesystem paths.
- APNG, animated WebP, video, audio, shaders, and 3D content.
- Unbounded decoded-frame or GPU memory.

### Pending technical decisions

_None._

### Acceptance criteria

- A valid GIF animates in a scene without client-driven frame swapping.
- Static textures and existing image nodes retain their behavior.
- Invalid, oversized, and malformed animation data fails safely per session.
- Resource cleanup releases all decoded and GPU state on node/session removal.
- Timing tests use a deterministic clock; manual playback is verified on Linux
  and Windows.

### Accepted decisions

- **Date:** 2026-10-05
  **Decision:** Preserve the stable RGBA8 `TextureHandle` API. Add the
  provisional `animated_images` capability, an `AnimatedImageHandle`, and an
  `upload_animated_image` command whose format is initially GIF and whose
  encoded bytes are a binary payload rather than a host path.
  **Context:** Static RGBA8 textures are already available to CLP v1.3 clients,
  while future APNG or WebP support needs a format-neutral animated resource.
  **Rationale:** Separate handles preserve existing behavior and make animated
  ownership explicit. Static and animated sources retain the same scene-image
  geometry, opacity, clipping, stacking, and interaction behavior.
  **Consequences:** Animated-image commands require `scene`, `resources`, and
  `animated_images`; future encoded animated formats extend the format enum,
  not node semantics.
  **Validation:** Capability negotiation and binary-payload tests must reject
  paths, unknown formats, malformed handles, and ungranted commands.

- **Date:** 2026-10-05
  **Decision:** Limit an encoded GIF upload to 16 MiB, an animation to 300
  frames and 64 MiB of decoded pixels, and all static plus decoded animated
  image data retained by a session to 128 MiB. Each decoded frame must also
  satisfy the active GPU dimension and 64 MiB texture limits.
  **Context:** Animated content can expand a small encoded input into many
  full RGBA8 frames and otherwise bypass the M12 texture quotas.
  **Rationale:** Reusing the established texture ceilings keeps memory and GPU
  pressure bounded while allowing practical terminal-scale animations.
  **Consequences:** Validation completes before ownership or GPU state is
  committed; a rejected upload leaves the session unchanged.
  **Validation:** Tests cover encoded-size, frame-count, dimension, decoded
  size, session-total, overflow, and cleanup accounting.

- **Date:** 2026-10-05
  **Decision:** Use a host-owned deterministic playback clock. GIF delays are
  clamped to 20 ms minimum and 10 s maximum; after a delayed host turn, advance
  at most one frame and schedule from the current time rather than catching up
  in a burst. Honor an explicit finite GIF loop count, treat zero as infinite,
  and, when no loop extension exists, play once and retain the final frame.
  **Context:** Zero-delay GIFs can spin indefinitely, while catch-up playback
  would create bursts after a resize, debugger stop, or overloaded machine.
  **Rationale:** The policy is predictable, bounds redraw work, and keeps a
  completed animation visible instead of clearing the node.
  **Consequences:** Client `timing` subscriptions do not control GIF playback.
  **Validation:** A fake clock covers zero-delay, clamping, finite and infinite
  loops, completion, and delayed host turns.

- **Date:** 2026-10-05
  **Decision:** Playback phase is global to an animated resource. All scene
  nodes referencing it display the same current frame; attaching another node
  never restarts it. If no visible node references the resource, playback
  pauses and preserves its current frame and remaining phase until visibility
  returns.
  **Context:** A shared retained resource must not duplicate frame buffers or
  acquire one independent clock per node.
  **Rationale:** Global phase makes sharing deterministic, while pausing fully
  invisible content avoids needless decode, upload, and redraw work.
  **Consequences:** Visibility affects playback only when every reference is
  invisible; node geometry and hit testing remain unchanged.
  **Validation:** Tests cover shared nodes, visibility transitions, phase
  preservation, and redraw scheduling.

- **Date:** 2026-10-05
  **Decision:** Program keyboard focus survives when its focused scene node is
  hidden or fully clipped. It is cleared by a primary click on terminal space
  or when the focused node is removed.
  **Context:** A program that temporarily hides all of its nodes must still be
  able to receive its own keyboard command to restore them.
  **Rationale:** Focus belongs to the program's explicit interaction state, not
  to the current paint visibility of one retained node.
  **Consequences:** Hidden animated nodes pause without preventing the focused
  program from showing them again. Dragging still cancels when its source is no
  longer painted.
  **Validation:** Interaction tests verify that hiding emits the expected hover
  leave while retaining program focus; node removal continues to clear focus.

- **Date:** 2026-10-05
  **Decision:** Destruction follows static texture rules: an animated resource
  cannot be explicitly destroyed while an owned scene node references it, and
  session close removes owned nodes before releasing all decoded and GPU state.
  **Context:** Reusing existing ownership semantics avoids dangling scene
  content and keeps cleanup independent between sessions.
  **Rationale:** Clients receive one predictable lifecycle for all image
  resources.
  **Validation:** Resource-in-use, node removal, session fault, disconnect,
  and cross-session ownership tests are required.

- **Date:** 2026-10-05
  **Decision:** M18 delivers host protocol support, deterministic tests, and
  an internal GIF probe. The public typed Rust SDK API remains provisional work
  for M19 consolidation.
  **Context:** M19 is the agreed point to freeze the combined v1.4 surface.
  **Rationale:** This allows the host design to mature without prematurely
  promising a public client API.
  **Validation:** The internal probe must demonstrate autonomous playback and
  cleanup on Linux and Windows before SDK consolidation.

### Validation record

- Automated: `cargo fmt --check`, `cargo check --all-targets`, focused
  animated-image tests, `cargo test --all-targets -- --test-threads=1`, and
  `cargo clippy --all-targets -- -D warnings` pass locally. The release binary
  and internal probe also build successfully. Unit and transport coverage
  verifies encoded binary framing, capability denial, bounded decode, malformed
  and truncated GIF rejection, transparent-frame composition with GIF disposal,
  stale resource IDs, sustained deterministic playback, shared phase,
  visibility pause/resume, one-frame delayed advancement, GIF repetition
  semantics, quota or handle-exhaustion rejection without allocation,
  cross-session ownership, focus preservation across hiding, node-reference
  destruction denial, and cleanup after node removal.
- Manual Linux: the user confirmed the animated-image probe operates correctly
  on 2026-10-05.
- Manual Windows: after a native VM release build and launch, the user
  confirmed the same probe operates correctly on 2026-10-05.

## M19 — CLP v1.4 and Rust SDK Consolidation

**Status:** Planned

### Scope

- Freeze the M16–M18 protocol design as CLP v1.4.
- Keep CLP v1.3 clients operational without access to new capabilities.
- Update `chymeral-rust-sdk` with typed text, animated-image, and external-drop
  APIs.
- Update protocol conformance, examples, documentation, and compatibility
  tests.
- Add or verify the SDK's Linux/Windows CI workflow for Rust 1.85 and stable.

### Required SDK coverage

- Handshake/version negotiation against v1.3 and v1.4 hosts.
- Capability denial and unsupported-feature errors.
- Text creation and intrinsic-bounds behavior.
- Animated-image upload, playback lifecycle, ownership, and cleanup.
- External-drop events, target ownership, and terminal/scene separation.
- Clean session close during each new operation.

### Planned examples

- `clp_conformance` extended for every v1.4 capability.
- `text_probe` for Unicode, styles, clipping, and calculated bounds.
- `animated_image_probe` for GIF playback and cleanup.
- `external_drop_probe` for manual desktop file-drop validation.

### Pending technical decisions

- Exact protocol-minor compatibility policy and public error vocabulary.
- SDK version number and release policy.
- Shared fixtures strategy between the core and separate SDK repository.
- CI integration strategy for cross-repository conformance.

### Acceptance criteria

- Core and SDK conformance pass on Linux and Windows.
- SDK CI runs format, tests, warnings-denied Clippy, and release examples on
  Rust 1.85 and stable.
- Public documentation describes every granted capability and its fallback.
- M16–M18 decisions are transferred to `Technical_Journal.md`.

### Accepted decisions

_None yet._

### Validation record

_Not started._

## M20 — Public Release Preparation

**Status:** Planned

### Scope

- Platform distribution, install/uninstall behavior, release assets, and
  checksums.
- Versioning, changelog, compatibility policy, and release notes.
- Long-running, multi-window, process-failure, and driver/platform validation.
- Final installation and troubleshooting documentation.

### Exclusions

- New graphics or protocol features.
- Additional language SDKs before the Rust reference remains conformant.

### Pending technical decisions

- Distribution formats for Linux and Windows.
- Signing policy for Windows artifacts.
- Whether the first public release is labelled preview, beta, or stable `0.1.0`.
- Support matrix for desktop environments and GPU drivers.

### Acceptance criteria

- A new user can install, configure, launch, and remove Chymeral using only
  public release materials.
- Release artifacts pass clean-environment checks on both supported platforms.
- Known limitations and support boundaries are documented clearly.

### Accepted decisions

_None yet._

### Validation record

_Not started._

## M21 — Language-Neutral SDK Ecosystem

**Status:** Deferred

### Scope

- Establish shared CLP fixtures or an IDL after v1.4 behavior is stable.
- Select the next SDK according to actual user demand.
- Keep the Rust SDK as a conformance reference, not a privileged runtime path.

### Prerequisite

M19 must be complete and the v1.4 protocol contract must be stable.
