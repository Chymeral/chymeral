# Chymeral Next Steps

This is the working execution record for the milestones that follow M14. It is
not the public product manual and it is not a historical record yet. Once a
milestone is accepted, its confirmed technical decisions and validation evidence
must be summarized into [Technical_Journal.md](Technical_Journal.md).

## Current baseline

- M1–M26 are complete; M25 is released after Linux and Windows acceptance.
- CLP v2.1 is frozen for the `1.5.0` release line. A v2.1 host continues to
  accept v2.0 clients without its additional capabilities.
- `chymeral-rust-sdk` `1.5.0` and `@chymeral/node-sdk` `1.5.0` are the
  accepted release lines for the public CLP v2.1 APIs. Later protocol and SDK
  publications belong to their respective feature milestones.
- M26 is accepted on `current-dev`. Its batching capability extends CLP and
  both supported SDKs; the unpublished development target is Chymeral
  `v1.7.0`, both SDKs `1.7.0`, and CLP `v2.4`. The released `main` line remains
  at Chymeral and SDK `1.5.0` over CLP `v2.1` until a separately approved M26
  release is prepared.
- M26.1 is the active internal maintainability pass after M26. It preserves
  the accepted Chymeral `1.7.0`, SDK `1.7.0`, and CLP `v2.4` behavior while
  improving source navigation, module boundaries, and rationale documentation.
- No phase below has started unless its status explicitly says otherwise.

### M25 release record

- **Chymeral:** M25 integrates the accepted M23–M24 capabilities as `v1.5.0`.
- **CLP:** the protocol advances from `v2.0` to `v2.1`. A v2.1 host continues
  to accept v2.0 clients, which simply cannot request the new capabilities.
- **Supported SDKs:** the Rust and Node.js/TypeScript SDKs advance to `1.5.0`
  with their finalized CLP v2.1 APIs. Their release notes state their behavior
  against a v2.0-only host and the migration path from their `1.0.x` lines.
- **Distribution:** M25 prepares portable Linux `x86_64` and Windows `x86_64`
  editions only. ARM64 is deliberately outside M25 because no native ARM test
  hardware or reproducible validation matrix is available.
- These remain independent version lines: Chymeral and both supported SDKs use
  `1.5.0` for this coordinated feature release, while CLP `v2.1` is the
  compatible wire expansion. The coordinated Chymeral and SDK `2.0.0` release
  belongs to M28, after the M26 presentation work and M27 platform work have
  been independently accepted.

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

**Status:** Public release delivered; follow-up hardening remains ongoing

### Scope

- Platform distribution, install/uninstall behavior, release assets, and
  checksums.
- Versioning, changelog, compatibility policy, and release notes.
- Long-running, multi-window, process-failure, and driver/platform validation.
- Final installation and troubleshooting documentation.

### Exclusions

- New graphics or protocol features.
- Additional language SDK work, which is tracked separately in M22.

### Pending technical decisions

- Signing policy for Windows artifacts.
- Support matrix for desktop environments and GPU drivers.

### Acceptance criteria

- A new user can install, configure, launch, and remove Chymeral using only
  public release materials.
- Release artifacts pass clean-environment checks on both supported platforms.
- Known limitations and support boundaries are documented clearly.

### Accepted decisions

- **Version identity:** The core source baseline is Chymeral `1.0.0`, paired
  with CLP v2.0, `chymeral-rust-sdk` `1.0.0`, and `@chymeral/node-sdk` `1.0.0`.
  Those four version lines have separate compatibility roles. The first public
  artifact release was accepted as stable under M21; later SDK versions retain
  their own independent version lines.
- **First public delivery:** Chymeral `v1.0.0` is distributed as portable
  Linux x86_64 and Windows x86_64 ZIP archives through GitHub Releases. Each
  archive contains a platform launcher, configuration template, icon, license,
  and documentation. The Linux launcher selects X11 and therefore requires
  Xwayland plus `DISPLAY` on a Wayland desktop.
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
  stable.
- **Public release (2026-10-18):** Chymeral `v1.0.0` was published with its
  portable Linux and Windows editions. The paired Rust SDK `1.0.0` was
  published to crates.io.

## M22 — Language-Neutral SDK Ecosystem

**Status:** Complete (2026-10-20)

### Scope

- Establish shared CLP fixtures or an IDL after v2.0 behavior is stable.
- Maintain the Rust SDK as a conformance reference and the Node.js and
  TypeScript SDK as an independently released client; neither has a privileged
  runtime path.
- Select additional SDKs according to actual user demand.

### Prerequisite

M20 must be complete and the v2.0 protocol contract must be stable.

### Delivered

- **Canonical CLP v2.0 fixtures (2026-10-20):**
  [`protocol/fixtures/v2.0`](protocol/fixtures/v2.0/README.md) supplies a
  versioned, language-neutral corpus for lifecycle, subscriptions, retained
  scene mutation, intrinsic text, overlap, RGBA8/GIF binary uploads,
  notifications, errors, and termination. The core validates its client frames
  against the actual wire types, binary lengths against the transport, and
  command responses against a prior request in the same vector.
  `SHA256SUMS` locks its exact bytes for vendor snapshots. It is a portable
  regression reference, not a network dependency, code generator, or
  replacement for live conformance.
- **Node.js and TypeScript SDK (2026-10-19):**
  [`@chymeral/node-sdk`](https://www.npmjs.com/package/@chymeral/node-sdk)
  `1.0.1` is released on npm with ESM support for Node.js `22` or newer. It
  implements the same public CLP v2.0 contract through local IPC and retains
  independent Linux and Windows quality checks.
- **Independent SDK validation:** The Rust SDK `1.0.0` remains the conformance
  reference without a privileged runtime path. Both SDKs vendor and SHA-check
  the same corpus, then replay every vector through their public APIs and own
  fake hosts in their Linux/Windows version matrices.

### Validation record

- The canonical corpus is parsed and checked in the core transport tests.
- Rust runs `fixture_corpus` on Rust 1.85 and stable for Linux and Windows;
  Node runs its equivalent test on Node.js 22 and 24 for both platforms.
- Rust and Node SDKs retain separate deterministic fake-host coverage and live
  conformance executables; neither imports the host implementation or another
  SDK.

## M23 — Frame-Paced Scene Presentation

**Status:** Complete (2026-10-22)

### Scope

- Measure end-to-end continuous-interaction latency: host input arrival,
  application notification delivery, command receipt, scene mutation, render
  submission, and presentation where the backend exposes that boundary.
- Benchmark retained-scene interactions that update a parent transform and a
  related control in response to high-rate pointer movement, scrolling, and
  resizing.
- Validate and, if necessary, extend continuous-notification coalescing so the
  newest pointer or scroll state wins under pressure while discrete input keeps
  its ordering and delivery guarantees.
- Add an opt-in, session-local frame-presentation path for bounded transient
  scene state. It must atomically commit related visual changes at a renderer
  frame without requiring a separate `set_node` acknowledgement per property.
- Ensure retained parent transforms, clipping, and sibling control updates do
  not rebuild or re-upload unaffected scene descendants.

### Exclusions

- A general widget, layout, or application framework.
- Artificial interpolation intended to hide stale input, or a fixed-FPS promise
  to CLP applications.
- Unbounded concurrent commands that weaken session ownership, command order,
  or deterministic scene mutation semantics.

### Accepted strategic decision

- **Date:** 2026-10-20
  **Decision:** Retain the current ordered CLP command path as the reliable
  control plane, and add a separate opt-in frame-presentation lane for
  high-frequency visual state in CLP v2.1.
  **Context:** A retained scene already gives Chymeral the correct ownership
  and mutation model for desktop applications. Treating every pointer sample
  as a command that must complete an IPC round-trip before the next sample is
  processed, however, makes continuous interaction feel delayed even when only
  a parent transform and a scrollbar thumb change.
  **Rationale:** Graphics engines keep structural commands reliable while
  presenting the newest available transient state once per display opportunity.
  Adopting that separation preserves CLP's deterministic control semantics
  without using a fixed-rate application tick or interpolation to mask stale
  input.
  **Consequences:** `create_node`, structural `set_node`, ownership, and
  discrete input remain ordered and acknowledged exactly as in CLP v2.0. The
  new lane is limited to documented presentational properties, coalesces stale
  continuous state, and commits each accepted packet atomically at a host frame
  boundary. It is not arbitrary concurrent command execution.
  **Validation:** A host trace, stress benchmark, SDK conformance fixture, and
  Linux/Windows manual profiling matrix must prove both the latency improvement
  and preservation of existing control-plane semantics.

### Accepted implementation decisions

- **Date:** 2026-10-21
  **Decision:** The first presentation lane changes geometry only: `x`, `y`,
  `width`, and `height`; intrinsic text accepts only `x` and `y`.
  **Context:** Continuous interaction needs parent translation and companion
  control movement without turning the lane into an unordered replacement for
  retained-scene commands.
  **Rationale:** Geometry is sufficient for panning, dragging, scrolling, and
  lightweight controls while preserving the ownership, resource, style,
  hierarchy, visibility, and interaction contracts of `set_node`.
  **Consequences:** A packet contains 1–256 unique owned node handles, has a
  non-empty UTF-8 identifier of at most 128 bytes, and has at most 64 KiB of
  JSON payload. Every update is validated before any geometry is changed;
  invalid packets leave the retained scene unchanged.
  **Validation:** Core unit tests cover text-bound exclusion, ownership,
  duplicate handles, non-finite geometry, and atomic rejection. The shared
  v2.1 fixture covers one accepted packet and one rejected text-bounds packet.

- **Date:** 2026-10-21
  **Decision:** Retain only the newest unpresented presentation packet per
  session. A later reliable scene mutation or a later committed presentation
  replaces any state that has not reached `queue.present`.
  **Context:** A redraw can be deferred or rejected by an outdated, occluded,
  or temporarily unavailable surface after a packet has been committed.
  **Rationale:** Reporting such state as presented on a later frame would lie
  about what the user could have observed and would reintroduce stale visual
  backlog.
  **Consequences:** `presented`, `superseded`, and `rejected` statuses are
  advisory coalesced notifications, not a per-packet acknowledgement queue.
  `presented` means Chymeral called `queue.present`; it does not promise
  physical scanout. Applications keep structural changes on the ordered
  command path and submit the newest transient state without waiting for every
  status.
  **Validation:** Transport tests prove mailbox replacement and absence of a
  command round trip; the Node presentation probe reports observed statuses
  during pointer, scroll, resize, and burst traffic.

- **Date:** 2026-10-21
  **Decision:** Trace only host-observable boundaries and label unavailable
  client-delivery and physical-scanout timing explicitly.
  **Context:** CLP applications can run in arbitrary runtimes and wgpu/window
  backends do not expose a portable scanout timestamp.
  **Rationale:** Invented end-to-end figures would make latency diagnostics
  less trustworthy than an honest staged trace.
  **Consequences:** `CHYMERAL_PRESENTATION_TRACE=1` reports contextual latest
  host input to transport receipt, transport receipt to commit, commit to
  submission, and transport/commit to the host `present` call. It writes
  `client_delivery_ms=unavailable` and `scanout_timing=unavailable` rather
  than claiming those boundaries were measured.
  **Validation:** The probe documents its application-submit-to-observed-status
  metric separately from the host trace and does not call either physical
  input-to-photon timing.

### Architecture and delivery strategy

1. **Measure before changing the wire.** Add correlation identifiers and
   timestamps for input receipt, CLP delivery, command/batch acceptance, render
   submission, and presentation where available. The benchmark must include a
   static terminal with a moving scene subtree and control, so terminal redraw
   cost cannot be confused with scene-update cost.
2. **Keep two explicit lanes.** The reliable lane continues to serialize and
   acknowledge structural work. The new presentation lane accepts only a
   bounded packet of transient state for one session; a newer pending packet
   replaces an older unpresented packet from that session.
3. **Commit once per renderer frame.** The host drains the latest eligible
   presentation packet at its redraw/vsync opportunity and applies the packet
   atomically. Applications receive bounded completion or error reporting for
   the packet, never a synchronous round-trip for every property mutation.
4. **Make presentation work cheap.** Parent transforms, clips, and dynamic
   controls must compose over cached terminal and unaffected scene content.
   The renderer must use dirty regions or retained GPU state rather than
   recreate terminal glyphs, scene nodes, or image uploads for those updates.
5. **Protect input freshness.** Pointer movement and scroll may coalesce to the
   newest state. Press, release, key, focus, close, ownership, and structural
   command events never take the lossy path or change relative order.
6. **Version and expose deliberately.** Freeze the capability, packet limits,
   eligible properties, completion semantics, and SDK APIs only after the host
   prototype and trace show the required behavior. M25 releases the result as
   Chymeral and supported SDK `1.5.0` lines over CLP v2.1; CLP v2.0 clients
   remain on the reliable path unchanged. M26 is planned as the additive CLP
   `v2.4` and coordinated `1.7.0` release, while M27 remains an independently
   gated platform milestone. Their coordinated SDK `2.0.0` publication is
   reserved for M28.

### Performance quality bar

- The host should use every practical display-refresh opportunity rather than
  impose an artificial update cap below the active surface's refresh rate.
- On agreed reference hardware, the interaction benchmark targets stable
  60 Hz-or-better presentation when the display supports it, with higher
  refresh rates exercised when available.
- The benchmark records the closest available host-to-presentation stage in
  refresh intervals. Client delivery and physical scanout are explicitly
  labelled unavailable where the backend cannot expose them. The initial target
  is p95 at or below two refresh intervals for that available host stage and no
  unbounded stale-input backlog; platform and driver limitations must be
  reported rather than hidden.
- This is a performance quality bar and regression gate, not a universal fixed
  FPS or latency guarantee to every CLP application.

### Acceptance criteria

- Diagnostics distinguish time spent in the host input queue, client delivery,
  IPC/command processing, renderer submission, and the nearest available
  presentation boundary; unavailable platform timing is labelled as such.
- Under sustained pointer movement, obsolete continuous updates do not create
  an unbounded visible-latency queue. The rendered position reflects the newest
  eligible state at each presentation opportunity.
- Press, release, key, close, ownership, and command ordering semantics remain
  unchanged while continuous notification or mutation coalescing is active.
- A parent translation plus its scrollbar/control position commits as one
  ordered visual update through the frame-presentation path, without rebuilding
  unaffected terminal or scene content.
- The documented benchmark meets the performance quality bar on designated
  reference hardware and reports results separately by backend and display
  refresh rate. Constrained virtual machines are functional-validation
  platforms, not performance gates.
- Automated stress coverage and manual functional validation pass on Windows
  and on Linux X11/Xwayland and native Wayland where the backend supports the
  required measurements.

### Completion record

- CLP v2.1 adds the negotiated `presentation` capability and its bounded,
  geometry-only `present_scene` mailbox. The reliable `set_node` control plane
  remains ordered and acknowledged; visual packets are atomic and newest-state
  wins before a renderer frame.
- `presentation_status` and `CHYMERAL_PRESENTATION_TRACE=1` deliberately
  report host-observable stages only. Status delivery is advisory and
  coalesced; neither the probe nor the trace claims physical scanout timing.
- Core formatting, tests, warnings-denied Clippy, release build, and the v2.1
  fixture corpus passed on Linux. Windows passed the regression test,
  warnings-denied Clippy, and a release build after the same source update.
  The Rust and Node SDKs passed their independent fixture and conformance
  coverage on both platforms.
- Manual Linux acceptance completed on native Wayland and forced X11/Xwayland.
  The native Wayland probe reported 2,314 submitted packets, 1,799 presented,
  513 superseded, zero rejected, and observed status p50/p95 of 30.23/40.44
  ms. The forced X11/Xwayland probe reported 1,654 submitted, 1,199
  presented, 455 superseded, zero rejected, and p50/p95 of 30.21/40.32 ms.
- The opt-in native-Wayland host trace measured transport-to-`present` p50/p95
  of 18.626/25.486 ms, with a 457.954 ms maximum during the explicitly traced
  interactive run. It excludes client delivery and physical scanout, so it is
  evidence for the host stage rather than an input-to-photon claim.
- The constrained Windows VM is accepted as a functional platform, not a
  performance reference: its probe completed cleanly with zero rejected
  packets and preserved the terminal after CLP closure. Its observed status
  timing is recorded separately because it is not physical scanout timing.
- Manual stress testing exposed a terminal scrollback resize defect: rows
  retained at a narrower historic width could be indexed using a newer wider
  grid width. Rendering, selection, and copy now bound each retained row by
  its own cell length; a regression test covers the case.

## M24 — CLP Application Window Metadata

**Status:** Complete (accepted 2026-10-22)

### Scope

- Add an optional `window_metadata` CLP capability in CLP v2.1.
- Let an owning application session declare its presented window title and a
  bounded static application icon so the native window and taskbar can identify
  the program rather than only Chymeral.
- Apply metadata for the lifetime of its declaring session only, with a clear
  fallback to Chymeral's terminal-derived title and bundled icon when metadata
  is cleared or the session closes.
- Expose the finalized operation and types consistently through the Rust and
  Node SDKs, shared fixtures, protocol documentation, and conformance probes.

### Exclusions

- Window position, size, fullscreen, opacity, decorations, menus,
  notifications, or arbitrary desktop-window management.
- Persistent launcher metadata, configuration-file changes, remote icon URLs,
  or host filesystem paths supplied by an application.
- Animated window icons or metadata ownership shared across sessions.

### Frozen protocol direction

- The capability will provide a session-owned metadata update operation
  (candidate name: `set_window_metadata`) with optional title/name and icon
  fields. Its exact wire shape and limits must be frozen with the v2.1
  specification.
- An application-owned title is presented verbatim while its metadata claim is
  active; it takes precedence over terminal OSC title updates. Clearing the
  claim or closing its session restores normal terminal-title behavior.
- Icon input must be a bounded static raster resource or payload owned by the
  declaring session. It must not fetch data or grant filesystem access. The
  protocol design must define whether this reuses a constrained image handle or
  introduces a dedicated icon resource.
- Metadata conflicts between concurrent sessions require a deterministic,
  documented ownership and restoration rule before implementation begins.
- CLP v2.0 clients neither request nor depend on this capability and continue
  to receive Chymeral's existing title and icon behavior on an updated host.

### Accepted implementation decisions

- **Arbitration:** Every session retains its own metadata claim. The newest
  successful nonempty claim is visible. Clearing or closing the active claim
  restores the newest live remaining claim; when none remains, Chymeral uses
  the terminal-derived title and bundled icon.
- **Patch semantics:** `set_window_metadata` is an ordered CLP v2.1 command.
  An omitted `title` or `icon` preserves that field, JSON `null` clears it,
  and a successful nonempty update refreshes the claim's recency. Both fields
  cleared remove the session claim. Titles are verbatim, nonempty Unicode of
  at most 256 UTF-8 bytes and cannot contain control characters.
- **Icon input:** `icon` is a session-owned static RGBA8 `image-N` handle, not
  a path, URL, GIF, or standalone payload. It is limited to 256 × 256 pixels
  and 256 KiB. A referenced icon cannot be removed until it is cleared or its
  session ends.
- **Backend feedback:** Native Wayland retains application titles but reports
  `icon_supported: false`; Chymeral does not force X11. Windows and supported
  Linux backends apply the dynamic icon and expose `icon_supported: true`.
- **Diagnostics:** A host diagnostics title temporarily wins while diagnostics
  are open, then normal active-claim arbitration resumes.

### Acceptance criteria

- Capability negotiation, title/icon validation, limits, session ownership,
  replacement, clearing, session closure, and v2.0 fallback have wire and
  conformance coverage.
- A program can present its own title without a forced `Chymeral —` prefix
  while its metadata claim is active.
- The native title and icon update correctly on Windows and on supported Linux
  backends. Backend or desktop-environment icon limitations, especially under
  Wayland, are detected or documented rather than silently misrepresented.
- Closing the declaring application restores terminal-derived title behavior
  and Chymeral's fallback icon without leaving process-specific metadata in
  later sessions.
- SDK examples demonstrate title-only metadata and title-plus-icon metadata
  without relying on platform-specific file paths.

### Acceptance record

- The core, Rust SDK, and Node SDK validation suites passed on the stable Rust
  toolchain; the canonical CLP v2.1 fixture mirrors and SHA-256 checks match
  byte-for-byte across all three repositories.
- Manual Node SDK probe acceptance confirmed native title and dynamic icon
  updates on Windows and on Linux forced to X11/Xwayland.
- Manual native-Wayland acceptance confirmed title updates and deterministic
  metadata restoration while reporting `icon_supported: false`, as documented.

## M25 — v1.5.0 Release Integration

**Status:** Complete — release-ready (validated 2026-10-22)

### Scope

- Publish the already accepted M23 frame-paced presentation and M24 application
  window metadata capabilities; M25 is a user-visible feature release, not a
  no-op packaging pass.
- Align the Chymeral application, `chymeral-rust-sdk`, and
  `@chymeral/node-sdk` on their respective `1.5.0` release lines while keeping
  CLP at the compatible `v2.1` wire revision.
- Finalize public manuals, changelogs, migration notes, release notes,
  versioned fixture provenance, and reproducible portable artifacts.
- Publish and verify Linux `x86_64` and Windows `x86_64` portable editions,
  the Rust crate, and the Node package only after their independent release
  quality gates pass.

### Exclusions

- No new CLP operations or behavior beyond the accepted M23/M24 contract.
- No `winit` beta, local windowing fork, native-Wayland DnD implementation, or
  outgoing desktop DnD. Those remain M27 work behind an upstream-stable gate.
- No ARM64 artifact or support claim. Native ARM validation requires actual
  hardware and is a future, separately scoped effort.
- No SDK `2.0.0` release: that coordinated API generation is deliberately
  deferred to M28 after the separately validated M26 and M27 work.

### Accepted release decisions

- **Version matrix:** Chymeral `v1.5.0`, `chymeral-rust-sdk` `1.5.0`, and
  `@chymeral/node-sdk` `1.5.0` expose CLP `v2.1`. The independently versioned
  protocol does not change again during M25.
- **Compatibility:** A v2.1 host continues to accept v2.0 clients without
  granting `presentation` or `window_metadata`; v2.1 SDK capability feedback
  remains the application-facing way to select the enhanced path.
- **Artifact matrix:** The public binary release consists of Linux `x86_64`
  and Windows `x86_64` portable ZIPs, their hashes, launchers, bundled icon,
  and default configuration. The Linux launcher keeps the established X11/
  Xwayland default; native Wayland remains usable when launched directly.
- **Publication boundary:** Version bumps, tags, registry publication, GitHub
  releases, and portable archives are executed only after all local and
  cross-platform validation evidence is recorded. Credentials and publication
  actions remain user-controlled.

### Execution checklist

1. Update application and SDK manifests, lockfiles where required, public
   version references, changelogs, migration notes, and release descriptions.
2. Run each repository's locked format, test, lint, documentation, package,
   and release-build gates; verify the mirrored v2.1 fixture corpus and its
   SHA-256 files remain byte-identical.
3. Rebuild and manually validate the release candidates on Linux native
   Wayland and forced X11/Xwayland, plus the Windows VM. Exercise a v2.0 client
   fallback and both v2.1 capabilities on every applicable backend.
4. Create the two portable `x86_64` archives from the validated binaries,
   inspect their launchers/configuration/icons, generate hashes, and perform
   a clean-directory smoke test for each platform.
5. Publish core artifacts and release notes, then the matching Rust and Node
   SDK `1.5.0` packages and releases; finally verify the public install and
   compatibility instructions from a clean consumer environment.

### Acceptance criteria

- Public documentation describes `presentation` and `window_metadata` as
  released CLP v2.1 features rather than development-preview behavior.
- The three `1.5.0` releases agree on their required host/protocol versions,
  capabilities, limits, fallback behavior, and migration guidance.
- Linux and Windows portable ZIPs are self-contained, branded, hash-verified,
  and successfully launch on their documented `x86_64` targets.
- Core and both SDKs pass their release quality gates, conformance checks, and
  the existing M23/M24 manual validation matrix without a regression.
- The technical journal records the final versions, validation evidence,
  artifact hashes, and any documented backend limitation before M25 is marked
  complete.

### Completion record

- Locked formatting, test, lint, Rustdoc, package, and release-build gates
  passed for the core and both SDKs. The three v2.1 fixture snapshots and their
  SHA-256 manifests were byte-identical.
- The final portable archives passed clean ZIP inspection and manual launch:
  `chymeral-1.5.0-linux-x86_64-portable.zip` has SHA-256
  `04bab1749a77e60b66a2b803b9e70868afe4d2852e6a70df6300062a2cf39e65`;
  `chymeral-1.5.0-windows-x86_64-portable.zip` has SHA-256
  `52d64ec1b944b80683147828004cb86484fd4043d7b427af72e2a30cb66640c6`.
- Manual Node SDK probes accepted presentation and window metadata on Windows,
  Linux forced to X11/Xwayland, and Linux native Wayland. Native Wayland
  updates titles and explicitly reports dynamic icons unsupported; incoming
  external file drops remain unavailable there by the existing M16 boundary.
- Credential use, tags, registry publication, and GitHub release creation were
  intentionally left to the repository owner after this release-ready state.

## M26 — Batched Scene Construction and Atomic First Presentation

**Status:** Complete (validated 2026-10-24 on `current-dev`; unreleased)

### Scope

- Add a bounded, reliable way for a CLP client to describe an initial retained
  scene or a structural scene revision as one validated unit.
- Commit an accepted unit without exposing its intermediate node-by-node
  construction to the user, then request one coherent first presentation.
- Preserve the existing ordered `create_node` and `set_node` path as an
  explicit application-controlled incremental alternative.
- Expose an ergonomic helper through supported SDKs so applications do not
  need to hand-roll a transport batch merely to avoid progressive first paint.
- Keep construction semantics distinct from renderer submission and physical
  scanout; the batch result requests one redraw but is not a timing promise.

### Rationale and boundary

M23's `presentation` mailbox is intentionally geometry-only and addresses
continuous updates to an already retained scene. It does not create nodes and
must not be widened implicitly to carry arbitrary structural work. M26 is a
separate experience modernization: it targets the visible progressive assembly
of a large client scene caused by serial, acknowledged node creation. It is not
a correction to Chymeral's retained renderer or a substitute for the current
robustness and conformance work.

### Accepted release decision

- M26 necessarily adds a negotiated CLP capability, wire fixtures, and public
  Rust and Node.js/TypeScript helpers. Once its implementation and validation
  are accepted, publish Chymeral `v1.7.0`, `chymeral-rust-sdk` `1.7.0`, and
  `@chymeral/node-sdk` `1.7.0` over CLP `v2.4`.
- The capability is additive: a v2.4 host continues to accept v2.0 and v2.1
  clients. An application without the new capability retains the ordinary
  ordered sequential path only when it explicitly chooses it; SDKs do not
  silently expand a batch into multiple commands.
- M26 is intentionally independent of the stable-Wayland entry gate. It may
  proceed now to support early application adoption without waiting for M27.

### Required guarantees

- Every staged operation is validated for capability, session ownership,
  hierarchy, resource limits, and handle references before any part becomes
  visible.
- An invalid unit leaves the live scene unchanged; no partial tree, leaked
  resource, or half-applied metadata may survive rejection.
- A valid unit preserves deterministic parent/child paint order, sibling
  ordering, clipping, alpha hit testing, overlap state, focus, and cleanup
  semantics.
- The host never weakens the existing ordered command guarantees for ordinary
  mutations, discrete input, or session teardown.
- Clients without the future capability remain fully functional through the
  existing sequential CLP v2.1 path.

### Implemented technical decisions

- **Negotiation and wire form:** CLP `v2.4` adds the additive `scene_batch`
  capability and a reliable `scene_batch` command. The command contains ordered
  `create_node`, `set_node`, and `remove_node` operations; it does not include
  resources, metadata, subscriptions, or presentation packets.
- **Bounds:** A batch has 1–512 operations, a complete JSON command envelope
  no larger than 512 KiB, and UTF-8 aliases of 1–128 bytes without control
  characters. An alias is unique, introduced by `create_node`, and may be used
  only by a later operation. This makes parent-before-child construction
  explicit and deterministic.
- **Atomic result:** The host snapshots the retained scene, node ownership,
  node allocator, and overlap-delivery state. It restores all of them on any
  rejection, reconciles only the final overlap state on success, and requests
  one redraw. The response includes ordered operation outcomes and handles for
  aliases that remain live; temporary created-and-removed aliases are omitted.
  This is not a compositor or physical-scanout guarantee.
- **SDK surface:** Rust exposes typed `SceneBatch`, `SceneBatchOperation`,
  `BatchNodeRef`, and `SceneBatchNodeChanges`; Node exposes the equivalent
  typed `SceneBatch` builder and types. Neither is a VDOM, toolkit, or host
  plugin. Both return a clear capability error when `scene_batch` is absent.
- **Conformance:** The core owns a CLP v2.4 fixture corpus. Both SDKs vendor
  its exact bytes and SHA-256 manifest, replay it through their public APIs,
  and retain v2.0-only fallback negotiation without requesting `scene_batch`.
  The Node SDK also carries a live `example:scene-batch` first-paint probe; it
  builds a visible nested panel from one batch without introducing a toolkit.

### Acceptance criteria

- A representative application can create its complete first scene without a
  user-visible partial assembly of labels, controls, images, or decorative
  layers.
- Automated host tests prove all-or-nothing visibility, ownership isolation,
  resource cleanup, malformed-unit rejection, and compatibility fallback.
- Rust and Node SDK conformance suites exercise the same fixture corpus and
  expose clear capability feedback when the host lacks the feature.
- Linux X11/Xwayland, native Wayland, and Windows manual tests distinguish
  initial construction latency from steady-state interaction latency.
- Documentation records that batching improves first-paint experience by
  reducing serial IPC round trips; it does not promise a fixed render time or
  hide slow application logic.

### Completion record

- Core formatting, locked tests, clippy, Rustdoc, and release build passed.
  Both SDKs passed their corresponding locked format, type/test, documentation,
  and release/package gates; all three repositories replay the reviewed v2.4
  fixture snapshot and SHA-256 manifest.
- The Node `example:scene-batch` probe was manually accepted on the Linux host
  and Windows VM. Its complete retained panel appears from one batch; a click
  anywhere on the panel background or a child card focuses the owning CLP
  session, acknowledges the input through `set_node`, and `q` closes only that
  session while leaving the terminal alive.
- Final Windows validation exposed the default 512-byte named-pipe output
  buffer: a 1,433-byte scene-batch response could return a zero-byte write
  forever when submitted as one transport write. `RunningFrameWriter` now
  emits every queued CLP frame in bounded 512-byte chunks. The wire frame and
  its atomic result remain unchanged; a regression test verifies a large frame
  completes through a bounded transport.

## M26.1 — Internal Maintainability Structure

**Status:** In progress; internal-only, no protocol or public API change

**Progress:** Baseline validation and the first core/SDK source extractions are
complete. The remaining work continues through small mechanical moves only.

### Purpose

Make the core and supported SDKs easier to navigate, review, extend, and
maintain without changing what a terminal user or CLP application observes.
This is a structural maintenance pass, not a feature milestone and not a
release-version change.

### Non-negotiable invariants

- Preserve the accepted CLP v2.4 wire form, capability negotiation, limits,
  fixture bytes, SHA-256 manifests, and all current error semantics.
- Preserve every public Rust and Node.js/TypeScript SDK export, method name,
  type shape, validation rule, and documented fallback behavior.
- Preserve terminal, scene, rendering, ownership, interaction, timing, and
  platform behavior. No visual redesign, performance feature, dependency
  upgrade, or new configuration setting belongs here.
- Keep every extraction mechanical and independently reviewable. A commit may
  move or clarify code, but it must not combine a refactor with a behavioral
  change.
- Retain current benchmark and regression coverage, including the real local
  transport large-frame test introduced during M26.

### Target source map

The following is an intended ownership map, not a requirement to make every
file equally small. A facade may remain where it improves discoverability.

#### Chymeral core

```text
src/
  transport.rs                 public host facade and narrow re-exports
  transport/
    framing.rs                 CLP length framing, nonblocking reads and writes
    wire.rs                    private serde wire schema and conversions
    notifications.rs           subscriptions, coalescing, delivery metrics
    server.rs                  listener, lifecycle, session-running loop
    tests.rs                   private transport unit and integration tests
  program_scene.rs             public retained-scene facade and re-exports
  program_scene/
    model.rs                   handles, inputs, patches, commands, results
    resources.rs               session ownership, quotas, images and cleanup
    batch.rs                   validation snapshot, rollback and commit logic
    execution.rs               ordinary command dispatch and presentation path
    tests.rs                   private scene-resource and batch tests
```

`src/main.rs` remains intact during the initial pass. Its `winit` lifecycle,
renderer setup, terminal integration, and platform hooks are closely coupled;
it is reconsidered only after the two high-value core facades are stable.

#### Rust SDK

```text
src/
  lib.rs                       documented public facade and re-exports
  types.rs                     public CLP model, handles and notifications
  client.rs                    public Client lifecycle and command methods
  wire.rs                      private serde wire schema and decoding
  framing.rs                   local-socket framing and I/O resilience
  tests.rs                     private unit tests
```

#### Node.js/TypeScript SDK

```text
src/
  index.ts                     stable public export facade
  types.ts                     public TypeScript contracts and limits
  client.ts                    connection state, command scheduling and events
  codec.ts                     CLP serialization and host-message decoding
  validation.ts                input, handle and capability validation
  transport.ts                 local-socket framing and I/O
  scene-batch.ts               focused SceneBatch builder
```

### Current implementation record

- The initial core extraction keeps the private wire schemas and exhaustive
  unit suites as separately navigable source files without widening their
  visibility. They are textually included by their existing private parent
  module, so malformed-frame and ownership tests retain the same access they
  had before the split.
- Narrow runtime units with stable internal contracts are real private modules:
  `transport/framing.rs` owns bounded local-socket framing, while
  `program_scene/batch.rs` owns the M26 snapshot, rollback, alias validation,
  and commit path.
- The Rust SDK root remains its documented public surface. Its client, wire
  schema, framing, and private tests are source-separated while retaining the
  crate-root API and Rustdoc paths.
- The Node.js SDK keeps `index.ts` as its only package entry point. The
  `Client` now owns lifecycle and scheduling, while `codec.ts` owns private
  CLP serialization, decoding, and validation. A later mechanical pass may
  split validation further only if it produces a clearer dependency direction.
- No extracted file is a newly supported import path or an addition to either
  SDK's public API.

### Execution order

1. **Baseline and boundaries**
   - Record the accepted validation commands and benchmark/probe baselines.
   - Add concise module-level responsibility maps and invariant comments where
     a subsystem boundary is non-obvious.
   - Do not add explanatory comments that merely restate local code.

2. **Private test extraction in the core**
   - Move the `#[cfg(test)]` bodies from `transport.rs` and
     `program_scene.rs` into sibling private test modules.
   - Preserve private access through `use super::*`; do not convert unit tests
     into weaker black-box tests merely to move files.
   - Validate the full core suite before and after each extraction.

3. **Core transport extraction**
   - Extract framing first, then notification delivery, then the private wire
     schema, and finally server/session execution.
   - Keep `transport.rs` as the stable internal entry point for `ClpServer`,
     `SessionEnvironment`, host actions, and notification metrics.
   - Make queue bounds, ordering, close behavior, and Windows named-pipe
     progress guarantees explicit at module boundaries.

4. **Core retained-scene extraction**
   - Extract public scene model types before resource ownership and execution.
   - Isolate scene-batch transaction/rollback logic as a named internal unit.
   - Keep ordinary command execution and presentation submission distinct;
     neither becomes a hidden batch or renderer abstraction.

5. **SDK internal extraction**
   - Split the Rust SDK without altering its crate-root public API or Rustdoc
     links.
   - Split the Node SDK codec and validation code from client orchestration
     without altering its package exports, generated declarations, or runtime
     wire validation.
   - Retain the vendored fixture corpus in each SDK so a checkout remains
     independently testable offline.

6. **Documentation and final review**
   - Add English comments only for rationale: concurrency, ownership,
     transaction boundaries, negotiated compatibility, platform constraints,
     and deliberately bounded queues.
   - Review naming, remove redundant helpers exposed by the extraction, and
     ensure modules depend inward rather than forming circular responsibilities.
   - Update `Technical_Journal.md` only with the final structural decisions,
     not a transcription of every file move.

### Review and validation gates

Each extraction step must pass its repository's normal format, test, lint,
documentation, and release-build/package checks. Before M26.1 is accepted:

- The core CLP v2.4 fixtures and both SDK copies remain byte-identical with
  matching SHA-256 manifests.
- The complete core, Rust SDK, and Node SDK conformance suites pass without
  changing expected wire transcripts.
- Existing M23 presentation, M26 scene-batch, and large-frame transport
  regressions remain covered and pass on Linux and Windows.
- A reviewer can follow a request from public SDK method to wire frame, host
  dispatch, scene mutation, and response without crossing unrelated concerns.
- `git diff --check` is clean and commits are grouped by one structural move
  rather than by broad rewrites.

### Accepted documentation policy

- Source comments and module docs are English, concise, and explain **why** an
  invariant or platform rule exists.
- Self-evident statements, line-by-line narration, and duplicated manual text
  are not added to source files.
- Public SDK documentation remains the source of API usage guidance;
  `Main_Manual.md` remains the source of terminal and host behavior guidance.

### Exclusions

- No CLP revision, SDK version bump, release, migration guide, or portable
  distribution change.
- No M27 Wayland capability work, dependency upgrade, or workaround.
- No M28 product consolidation, toolkit, widget abstraction, or new client
  language support.
- No Foregit changes; it is an independent application and may adopt the
  resulting source conventions later.

## M27 — Stable Wayland Capability Upgrade

**Status:** Deferred pending an upstream stable `winit` API

### Entry gate

- Begin only after a non-prerelease `winit` release provides the public,
  maintained Wayland interfaces needed for the intended desktop-data-transfer
  work and can be evaluated on real native-Wayland sessions.
- Do not use a beta dependency, private API, forced Xwayland workaround, or
  local `winit` fork merely to advance this milestone.

### Intended scope after the gate is met

- Upgrade the windowing stack deliberately and revalidate every existing
  Linux, Windows, X11/Xwayland, and native-Wayland behavior.
- Evaluate native-Wayland incoming external file drops, outgoing external DnD,
  and dynamic application-icon support against the actual stable upstream API.
  Only capabilities proven available and portable enough for a clear contract
  become CLP features.
- Freeze any resulting protocol/SDK changes with new fixtures, limits,
  ownership rules, fallback feedback, and real-desktop validation.
- Deliver accepted platform behavior as an independently validated capability
  baseline for the later M28 coordinated release; M27 itself does not publish
  Chymeral or the SDKs as `2.0.0`.

### Release boundary

- M27 follows M26 and does not implicitly revise the accepted M26 `1.7.0` /
  CLP `v2.4` contract. Any later protocol or SDK version is decided only after
  the stable upstream API and real-desktop behavior are known.

### Non-commitments

- M27 does not promise that every compositor will display a dynamic icon or
  accept every desktop DnD form; those outcomes depend on the verified stable
  API and compositor behavior.
- Its protocol revision, exact SDK surface, release date, and ARM support are
  intentionally undecided until the entry gate and implementation evidence
  exist. Its accepted outcome informs M28 but does not by itself force a
  product-major version bump.

## M28 — Chymeral 2.0 and SDK 2.0 Consolidation

**Status:** Planned; begins only after M26 and M27 are accepted

### Scope

- Consolidate the accepted platform and first-presentation work into the mature
  retained-2D runtime baseline for Chymeral `2.0`.
- Publish coordinated `2.0.0` releases of the supported Rust and Node SDKs,
  with fixtures, documentation, examples, migration notes, and capability
  feedback aligned to the accepted host contract.
- Define a long-lived compatibility policy for the stable 2D runtime,
  including how later optional capabilities can evolve without turning the
  host into a widget toolkit or breaking existing CLP applications.
- Revalidate the complete Linux and Windows matrix, including supported
  Wayland, X11/Xwayland, and fallback behavior, before any public major
  release claim.

### Product boundary

M28 establishes the definitive Chymeral 2D form: a conventional terminal with
a reusable local graphical runtime, retained scene graph, resources, input,
metadata, and explicit performance behavior. It does not claim that graphical
work ends forever. Future technologies such as specialized visualization,
shader, or three-dimensional capabilities must arrive as separately negotiated
and versioned extensions rather than a rewrite of this baseline.

### Acceptance criteria

- Chymeral, the Rust SDK, and the Node SDK agree on the final version matrix,
  supported CLP revisions, capability fallback, error vocabulary, limits, and
  migration path from the `1.x` release lines.
- The public contract is complete enough for third-party client libraries and
  optional toolkits to build sophisticated compositions above SDK structures
  without requiring host plugins or new core widget abstractions.
- Release-quality conformance, security, regression, performance, packaging,
  documentation, and real-desktop validation pass on the declared platforms.
- The release notes distinguish the stable 2D baseline from later optional
  graphical technologies and from terminal-experience work.

## M29 — Terminal Sessions and Tabs

**Status:** Planned after M28; intentionally a terminal-product milestone

### Scope

- Add host-managed tabs for convenient use of multiple conventional terminal
  sessions within one Chymeral window.
- Give each tab its own PTY, shell/process lifetime, terminal state, scrollback,
  and associated CLP session when that session's process elects to use the
  runtime.
- Show the active tab's terminal and scene surface while preserving the existing
  ability for its CLP application to cover the full Chymeral window and control
  its title and icon where the platform permits.
- Keep ordinary separate Chymeral windows as the simple answer when unrelated
  processes should not be grouped in one terminal window.

### Explicit non-goals

- No nested application windows, host widget framework, client-controlled tab
  manager, or new inter-session composition system.
- No artificial containment that prevents an active CLP application from using
  the full window surface or window metadata already granted to its session.
- No promise of split panes, session orchestration, process supervision, or
  workspace semantics; those require separate product decisions if ever needed.

### Decisions deferred to implementation

- Tab creation and close affordances, default shell/profile behavior, inactive
  tab redraw policy, and whether closing a window with live sessions requires
  confirmation.
- The exact handoff rules for terminal-derived versus active-CLP window
  metadata when changing tabs, including platform-specific icon fallbacks.
- Whether M29 changes CLP at all; the default direction is to keep it a host
  terminal feature rather than a new application-facing window-management API.

### Acceptance criteria

- Multiple tabs run independent conventional shell sessions without mixing
  stdin, stdout, stderr, scrollback, resize state, CLP tokens, or scene
  ownership.
- Switching tabs reliably reveals the active terminal/scene surface and restores
  terminal-derived or active-session branding deterministically.
- Closing one tab cleans only its owned PTY, process, CLP session, scene nodes,
  and resources; existing single-window close semantics remain predictable.
- Linux and Windows manual tests cover ordinary shells, a full-window CLP
  application, tab switching, background process behavior, and teardown.
