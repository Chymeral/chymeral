# Chymeral Next Steps

This is the working execution record for the milestones that follow M14. It is
not the public product manual and it is not a historical record yet. Once a
milestone is accepted, its confirmed technical decisions and validation evidence
must be summarized into [Technical_Journal.md](Technical_Journal.md).

## Current baseline

- M1–M20 are complete; M21 is the next planned phase.
- CLP v2.0 is frozen and accepted for its first public release preparation.
- `chymeral-rust-sdk` `1.0.0` is the accepted stable reference API; its release
  tag and any later package publication belong to M21.
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
M17, and M18 introduced a provisional v1.4 scene/protocol expansion. M19
replaces that provisional plan with the first public **CLP v2.0** contract.

The M19 contract intentionally breaks the v1.3 and provisional v1.4 wire
surfaces. There are no protocol aliases or compatibility shims: all future SDKs
bind directly to the v2.0 vocabulary and semantics.

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

## M19 — CLP v2.0 and Rust SDK Consolidation

**Status:** Complete

### Scope

- Replace CLP v1.3 and the provisional v1.4 surface with a deliberately
  breaking CLP v2.0 contract.
- Make the v2.0 canonical wire vocabulary the direct reference for every SDK.
- Update `chymeral-rust-sdk` to a v2.0-facing `0.2.0` API.
- Unify static RGBA8 and GIF-backed animated resources as client-visible images.
- Update protocol conformance, examples, documentation, and migration tests.
- Add or verify the SDK's Linux/Windows CI workflow for Rust 1.85 and stable.

### Canonical v2.0 operations

```text
create_node              set_node              remove_node
upload_image             remove_image
is_overlapping           get_overlaps
```

`create_node` accepts an optional parent and an optional root layer, so it
creates either roots or children. `set_node` atomically applies valid changes
to one node. `is_overlapping` is the direct boolean query; `get_overlaps`
returns the owned intersecting node handles when an application needs them.

Each CLP session owns one implicit retained scene tree. M19 does not add a
`create_scene_tree` wire operation: that name would incorrectly imply that a
session can create another scene. A future SDK-only declarative tree helper may
be added without changing CLP.

### Canonical v2.0 model

- Nodes are `empty`, `rectangle`, `image`, or `text`.
- A text node retains its intrinsic layout family. A visual node may change
  among `empty`, `rectangle`, and `image`, but it cannot become text; create a
  new node when changing between those families.
- `set_node` covers geometry, visibility, clipping, stacking, parenting,
  overlap participation, external-drop targeting, visual content, and text or
  text-style changes where valid for the node family.
- `upload_image` accepts `rgba8` and `gif` sources and returns one opaque
  `ImageHandle`; `remove_image` releases an unreferenced owned image.
- `images` replaces the v1.3 `resources` and provisional v1.4
  `animated_images` capabilities. The resulting v2.0 capability set is
  `scene`, `images`, `input`, `timing`, `overlap`, `external_drop`, and `text`.
- Incoming external drop remains capability- and node-opt-in based; it is an
  event family, not a scene command.

### Required SDK coverage

- Handshake/version rejection for legacy CLP and successful v2.0 negotiation.
- Canonical-operation, capability-denial, and stable error-code coverage.
- Creation and atomic mutation of every node family, including intrinsic text.
- Unified-image upload, playback lifecycle, ownership, and cleanup.
- External-drop events, target ownership, and terminal/scene separation.
- Clean session close during each new operation.

### Planned examples

- `clp_conformance` covers every v2.0 capability and operation.
- `text_probe` for Unicode, styles, clipping, and calculated bounds.
- `animated_image_probe` for GIF playback and cleanup.
- `external_drop_probe` for manual desktop file-drop validation.

### Finalized wire contract

`create_node` uses `{ "op": "create_node", "parent"?: "node-N",
"layer"?: "underlay" | "overlay", "node": ... }`. Omitted `parent` creates
a root; omitted root `layer` defaults to `overlay`; a child must omit `layer`.
`node` is tagged `empty`, `rectangle`, `image`, or `text`.

`set_node` uses `{ "op": "set_node", "node": "node-N", "changes": ... }`.
Every field in `changes` is optional: `properties`, `layer`, `visual`, `text`,
and `text_style` preserve their existing values when omitted. A structural
change is unambiguous because `parent` is either `{ "kind": "root" }` or
`{ "kind": "node", "node": "node-N" }`; omission means no parent change.
`visual` is tagged `empty`, `rectangle`, or `image`.

`upload_image` takes `source: { "kind": "rgba8", "width": W,
"height": H }` or `source: { "kind": "gif", "byte_length": N }`, followed
by exact consecutive binary bytes. The stable result tags are `node`,
`node_updated`, `image`, `is_overlapping`, `overlaps`, and `acknowledged`.

The stable v2 command-error vocabulary is `unsupported_version`,
`capability_denied`, `invalid_command`, `invalid_handle`, `not_owner`,
`image_in_use`, `resource_limit`, `invalid_argument`, `invalid_node_change`,
`host_capacity`, `host_unavailable`, and `protocol_violation`.

### Acceptance criteria

- Core and SDK v2.0 conformance pass on Linux and Windows.
- SDK CI runs format, tests, warnings-denied Clippy, and release examples on
  Rust 1.85 and stable.
- Public documentation describes every v2.0 operation, capability, limit, and
  backend fallback.
- M16–M18 decisions are transferred to `Technical_Journal.md`.

### Accepted decisions

- **Date:** 2026-10-06
  **Decision:** M19 replaces the planned CLP v1.4 freeze with a breaking CLP
  v2.0 contract. CLP v1.3 and provisional v1.4 clients are not supported by
  the v2.0 host.
  **Context:** All existing protocol use is test-stage work, so retaining a
  less coherent surface would impose permanent compatibility cost without a
  user benefit.
  **Rationale:** A major version creates one clear, language-neutral source of
  truth before external SDKs or applications adopt the protocol.
  **Consequences:** The host, SDK, examples, fixtures, documentation, and
  conformance tests move together. No wire aliases are added.
  **Validation:** Legacy-version rejection and v2.0 negotiation are tested on
  Linux and Windows.

- **Date:** 2026-10-06
  **Decision:** The canonical v2.0 operation vocabulary is `create_node`,
  `set_node`, `remove_node`, `upload_image`, `remove_image`,
  `is_overlapping`, and `get_overlaps`.
  **Context:** SDK authors need direct, understandable protocol operations to
  bind in their language rather than reverse-engineering host-internal terms.
  **Rationale:** The vocabulary describes the retained scene model while
  leaving language-specific casing and ergonomics to each SDK.
  **Consequences:** Root/child, static/animated-image, and overlap-query
  variants are consolidated under their canonical operations.
  **Validation:** Every operation has wire, host, SDK, conformance, and
  capability-denial coverage.

- **Date:** 2026-10-06
  **Decision:** A session owns one implicit scene tree. `create_node` selects a
  root or child through an optional parent; M19 does not add a
  `create_scene_tree` operation.
  **Context:** A separate tree-creation operation would misrepresent the
  session model and prematurely freeze bulk-transaction semantics.
  **Rationale:** Incremental retained-node mutation is sufficient for dynamic
  applications, while a later SDK-only declarative builder remains possible.
  **Consequences:** Root layer is valid only for parentless nodes.
  **Validation:** Conformance covers roots, children, reparenting through
  `set_node`, deterministic ordering, and subtree removal.

- **Date:** 2026-10-06
  **Decision:** v2.0 exposes four node families: `empty`, `rectangle`,
  `image`, and `text`. Text is intrinsic and cannot change into a visual node;
  visual nodes may change among the other three visual forms.
  **Context:** Text has host-computed bounds and distinct validation, whereas
  empty, rectangle, and image nodes share explicit rectangle geometry.
  **Rationale:** This preserves intuitive mutation without weakening text
  layout guarantees.
  **Consequences:** `set_node` validates the final node family atomically.
  **Validation:** Tests cover valid visual mutation and rejected cross-family
  text conversions.

- **Date:** 2026-10-06
  **Decision:** v2.0 unifies client-visible RGBA8 and GIF resources as opaque
  images. `upload_image` and `remove_image` replace texture and animated-image
  operations; `images` replaces `resources` and `animated_images`.
  **Context:** Both resource kinds occupy the same scene role and differ only
  in host decoding and playback details.
  **Rationale:** One image handle and one node-content form simplify every SDK
  without relaxing bounded upload, ownership, or cleanup rules.
  **Consequences:** Initial formats are `rgba8` and `gif`; future formats extend
  image source metadata rather than node semantics.
  **Validation:** Conformance covers both formats, shared GIF phase, limits,
  reference denial, cleanup, and cross-session ownership.

- **Date:** 2026-10-06
  **Decision:** The Rust SDK releases its breaking v2.0 binding as crate
  version `0.2.0`, remains unpublished on crates.io until M20, and uses the
  same canonical operation names in Rust snake case.
  **Context:** The SDK is a separate reference implementation and must not tie
  its crate version to the protocol major mechanically.
  **Rationale:** The version communicates an intentional pre-1.0 API break
  while retaining Git or path consumption during validation.
  **Consequences:** Existing v1.3 SDK examples and tests are replaced rather
  than compatibility-wrapped.
  **Validation:** Rust 1.85 and stable CI run format, tests, warnings-denied
  Clippy, release examples, and cross-repository conformance.

- **Date:** 2026-10-06
  **Decision:** Canonical language-neutral v2.0 fixtures live in the core
  repository. Cross-repository CI checks out fixed core and SDK revisions and
  runs the SDK conformance client against a real non-graphical CLP server;
  graphical GIF and external-drop behavior is additionally validated manually
  on Linux and Windows.
  **Context:** A standalone fixture repository would add lifecycle overhead
  before a second non-Rust SDK exists, while mock-only SDK tests cannot prove
  host compatibility.
  **Rationale:** The core owns the protocol and can provide the authoritative
  fixture and server behavior without adding a third project.
  **Consequences:** CI proves framing, lifecycle, commands, ownership, and
  negotiated capabilities; desktop integration remains an explicit matrix.
  **Validation:** The pinned cross-repository job and Linux/Windows manual
  validation both pass before M19 closes.

- **Date:** 2026-10-17
  **Decision:** `set_node` uses a single optional-field patch with tagged
  parent and visual forms; it never overloads JSON `null` to mean a root.
  **Context:** The protocol must distinguish an omitted parent update from a
  request to make a node root, without forcing SDKs to preserve language-
  specific nested-option behavior.
  **Rationale:** Tagged forms are readable in raw CLP, map predictably to every
  JSON SDK, and keep the command's atomic scope explicit.
  **Consequences:** Root/child changes, visual replacement, and intrinsic text
  changes all route through `set_node`; legacy mutation commands have no v2
  aliases.
  **Validation:** Core and SDK serialization tests cover the exact operation
  names, tagged shapes, capability gates, and rejected cross-family changes.

- **Date:** 2026-10-17
  **Decision:** Freeze the v2 command-error codes listed in the finalized wire
  contract above.
  **Context:** SDKs need reliable machine-readable outcomes while the host can
  continue improving human diagnostic messages.
  **Rationale:** A short stable vocabulary supports portable error handling and
  conformance testing without exposing Rust error types or renderer internals.
  **Consequences:** SDKs branch on `code`, not message text. New host detail is
  represented by a message or a future major/minor contract decision.
  **Validation:** Host dispatch maps all scene/image validation failures to the
  frozen vocabulary and the public manual records it.

- **Date:** 2026-10-17
  **Decision:** Retire the core repository's executable CLP fixture and
  animated-image probe after M19 validation.
  **Context:** They served as development subprojects, not product components
  or public SDKs, and would add unrelated executable artifacts to distribution
  work.
  **Rationale:** The core keeps its automated unit and integration tests while
  the Rust SDK retains its protocol-conformance program. A future
  language-neutral fixture or IDL belongs to M22 rather than to the
  distributable host repository.
  **Consequences:** The SDK no longer dispatches a cross-repository job against
  a core example. Host/SDK compatibility is recorded by the completed Linux
  and Windows conformance validation until M22 establishes its successor.
  **Validation:** No operational core documentation or SDK workflow references
  the retired executable examples.

### Validation record

- Automated core: `cargo fmt --check`, `cargo test --all-targets --
  --test-threads=1`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo build --release --examples` pass with 136 library tests, 65 binary
  tests, and the inherited-process CLP v2 client.
  Targeted tests cover v1-major rejection, capability negotiation, RGBA8/GIF
  image framing, canonical result forms, text metrics, overlap queries, unified
  image ownership, and rejected partial cross-family `set_node` changes.
- Automated SDK: `cargo fmt --check`, `cargo test --all-targets --
  --test-threads=1`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo build --release --examples` pass for the 0.2.0 client, wire
  serialization, and all examples.
- Cross-repository fixture (retired after validation): it launched the SDK's
  release `clp_conformance` example successfully on the Linux build host. It
  exercised real v2.0 framing and lifecycle plus every canonical operation,
  both image source forms, text, timing, and overlap.
- Manual Linux: on 2026-10-17, the user confirmed the current release host and
  Rust SDK `clp_conformance` client completed successfully in a graphical
  Chymeral session. The terminal remained open and reported exit code `0`.
- Manual Windows: on 2026-10-17, the same graphical conformance client passed
  in the VM and reported `CLP v2.0 conformance client passed.` with exit code
  `0`. An initial failure was diagnosed as a stale `chymeral.exe` from before
  the v2 implementation; a clean native rebuild produced the validated host.
- M16–M18 drag, text, animated-image, and external-drop behavior retain their
  prior Linux and Windows manual validation records. The SDK workflow provides
  a Rust 1.85/stable Linux/Windows quality matrix.

## M20 — Interaction Fidelity and Stable Rust SDK

**Status:** Complete

### Scope

- Add an optional alpha-derived contact shape to image nodes for pointer
  routing, external-drop targeting, overlap queries, and overlap transitions.
- Add generic node-level input passthrough so a visible node can paint above a
  control without intercepting its pointer-derived interaction.
- Preserve current rectangle-based contact and interactive behavior as the
  defaults for all existing v2.0 nodes.
- Validate static RGBA8 and animated GIF alpha contact on Linux and Windows.
- Promote `chymeral-rust-sdk` from the pre-release `0.2.0` API to its first
  stable `1.0.0` API after protocol, conformance, and desktop validation pass.

### Exclusions

- Arbitrary vector, polygon, physics, or 3D collision shapes.
- Alpha-derived shapes for rectangle, empty, or text nodes.
- Input passthrough that automatically suppresses a node's descendants.
- Package distribution and installer work; that follows in M21.

### Frozen technical decisions

- Alpha contact is an opt-in image-node property. It samples the source alpha
  of RGBA8 images or the current decoded GIF frame after a rectangle broad
  phase; it does not read pixels back from the GPU.
- The alpha threshold defaults to `1`: alpha `0` passes through and every
  visible alpha value participates. Clients may choose a threshold from
  `1` through `255`.
- Alpha contact affects pointer hover, press/release, click, drag/drop targets,
  external-drop targets, `is_overlapping`, `get_overlaps`, and overlap
  enter/leave transitions.
- `input_passthrough` is an opt-in property of every node family. It excludes
  only that node from pointer-derived targeting; its descendants retain their
  own independent interaction behavior and overlap participation is unchanged.
- These additions remain part of **CLP v2.0**. They are completed before the
  first public v2.0 release, so no `2.1` protocol revision or compatibility
  period is introduced.
- The Rust SDK's first public stable release is `1.0.0` after M20 acceptance.

### Acceptance criteria

- Default nodes preserve current rectangular interaction and overlap behavior.
- Alpha images route events and external drops through fully transparent pixels
  to the next eligible scene node or terminal surface.
- Alpha overlap produces deterministic query and transition results for static
  RGBA8 images and changing GIF frames, without unbounded per-frame work.
- `input_passthrough` works for every node family, preserves paint order, and
  leaves eligible descendants interactive.
- Core tests cover geometry, clipping, layering, GIF frame changes, ownership,
  cleanup, and interaction routing; Linux and Windows manual validation pass.
- The Rust SDK exposes the finalized features, passes its quality matrix and
  conformance program, and is ready to be tagged as `1.0.0`.

### Accepted decisions

- **Date:** 2026-10-17
  **Decision:** Complete alpha-derived image contact and generic node input
  passthrough before distribution work, without changing the CLP v2.0 version.
  **Context:** These capabilities belong to the already-agreed retained 2D
  scene model and were deferred during earlier feature milestones. No public
  v2.0 SDK or protocol release exists yet.
  **Rationale:** Alpha contact makes transparent sprites and images spatially
  truthful; passthrough permits decorative or layered presentation without
  duplicating the behavior of the interactive node beneath it.
  **Consequences:** M21 receives a finalized v2.0 contract and the Rust SDK can
  make its first stable `1.0.0` commitment after M20 validation.
  **Validation:** Automated conformance plus manual Linux and Windows scene
  tests must cover both features together and independently.

### Validation record

- **Automated core validation (Linux, 2026-10-17):** `cargo fmt --check`,
  `cargo test --all-targets -- --test-threads=1`,
  `cargo clippy --all-targets -- -D warnings`, and `cargo build --release`
  passed. Coverage includes static alpha hit testing and overlap, passthrough
  routing, alpha-aware external-drop targeting, wire conversion, and GIF frame
  contact reconciliation.
- **Automated SDK validation (Linux, 2026-10-17):** `cargo fmt --check`,
  `cargo test --all-targets -- --test-threads=1`,
  `cargo clippy --all-targets -- -D warnings`, and
  `cargo build --release --example clp_conformance` passed for
  `chymeral-rust-sdk` `1.0.0`.
- **Manual Linux acceptance (2026-10-17):** The user confirmed the M20 scene
  on the Fedora host: static alpha-frame versus transparent-center routing,
  visible passthrough routing, and GIF alpha-frame changes all worked. Native
  Wayland correctly withheld the pre-existing incoming external-drop capability.
- **Manual Windows acceptance (2026-10-17):** The user confirmed the same M20
  scene in the graphical VM after native core and SDK builds. Static alpha,
  passthrough, animated alpha contact, and session-only closure all worked.

## M21 — Public Release Preparation

**Status:** In progress

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
- Whether the first public artifact release is labelled preview, beta, or
  stable, despite the accepted `1.0.0` source baseline.
- Support matrix for desktop environments and GPU drivers.

### Acceptance criteria

- A new user can install, configure, launch, and remove Chymeral using only
  public release materials.
- Release artifacts pass clean-environment checks on both supported platforms.
- Known limitations and support boundaries are documented clearly.

### Accepted decisions

- **Version identity:** The core source baseline is Chymeral `1.0.0`, paired
  with CLP v2.0 and `chymeral-rust-sdk` `1.0.0`. Those three version lines have
  separate compatibility roles. This decision does not mark distribution
  artifacts as released; M21 still owns packaging, signing, installation, and
  clean-environment acceptance.
- **Core distribution boundary:** Chymeral is delivered as an application, not
  as a supported Rust host library. Its manifest uses `publish = false`; CLP
  and language SDKs are the application-development boundaries.
- **Toolchains:** The core's minimum supported Rust version is `1.90`, derived
  from its locked `wgpu` graphics stack. The independent Rust SDK keeps its
  tested Rust `1.85` minimum. Both repositories validate their minimum and
  current stable Rust on Linux and Windows in CI.
- **Review baseline:** Both repositories require locked dependency resolution,
  formatting, all-target tests, warnings-denied Clippy, warnings-denied Rustdoc,
  and a release build before a change is ready for review.

### Validation record

- **Source/review preparation (2026-10-17):** Both repositories gained pinned
  toolchains, locked quality commands, changelogs, contributor guidance,
  security-reporting policy, and Linux/Windows CI matrices for their minimum
  supported Rust and stable Rust. The core passed its full local matrix on Rust
  `1.90` and stable; the SDK passed its full local matrix on Rust `1.85` and
  stable. GitHub-hosted matrix runs remain pending the next push.

## M22 — Language-Neutral SDK Ecosystem

**Status:** Deferred

### Scope

- Establish shared CLP fixtures or an IDL after v2.0 behavior is stable.
- Select the next SDK according to actual user demand.
- Keep the Rust SDK as a conformance reference, not a privileged runtime path.

### Prerequisite

M20 must be complete and the v2.0 protocol contract must be stable.
