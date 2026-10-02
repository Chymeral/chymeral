# Chymeral Next Steps

This is the working execution record for the milestones that follow M14. It is
not the public product manual and it is not a historical record yet. Once a
milestone is accepted, its confirmed technical decisions and validation evidence
must be summarized into [Technical_Journal.md](Technical_Journal.md).

## Current baseline

- M1–M15 are complete.
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
`external_drop`, `text`, and `animated_image`.

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

**Status:** Planned

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

- CLP representation of dropped paths: URI form, path form, or a canonical
  representation that preserves non-UTF-8 Linux filenames.
- Terminal quoting policy: platform defaults plus a configuration override for
  custom shells.
- Target precedence when a scene node overlaps the terminal grid.
- Eligibility property and final event/handle names for CLP v1.4.
- Exact behavior for a drop over an unfocused Chymeral window.

### Acceptance criteria

- A dropped file is inserted as text only; no command is executed implicitly.
- POSIX and `cmd` quoting produce usable paths containing spaces and quotes.
- Only the owning opt-in session receives a scene drop event.
- Dropping over no eligible node falls back to terminal insertion.
- Core tests cover routing, ownership, quoting, and malformed data.
- Manual file-drop tests pass on Windows, X11, and Wayland where supported.

### Accepted decisions

_None yet._

### Validation record

_Not started._

## M17 — Intrinsic Scene Text

**Status:** Planned

### Scope

- Add scene text nodes with intrinsic bounds calculated from occupied text
  cells.
- Use terminal font, foreground color, cell width, and line height when a text
  style does not override them.
- Support one style per text node initially: font family, size, line height,
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

### Pending technical decisions

- Empty-text and trailing-newline bounds semantics.
- Tab expansion and tab-stop policy for scene text.
- Initial alignment, overflow, and clipping behavior.
- How custom font family fallback is made deterministic across platforms.
- Public text command/type shape for CLP v1.4 and the Rust SDK.

### Acceptance criteria

- Intrinsic bounds are deterministic for ASCII, wide Unicode, combining marks,
  tabs, and explicit line breaks.
- Default text visually follows configured terminal appearance.
- Text updates reconcile scene interaction and overlap correctly.
- Parent clipping and layer ordering match existing scene-node behavior.
- Unit tests cover geometry; visual probes pass on Linux and Windows.

### Accepted decisions

_None yet._

### Validation record

_Not started._

## M18 — Animated Scene Images

**Status:** Planned

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

- Per-resource frame, dimension, decoded-memory, and GPU-memory limits.
- Frame-duration clamping, loop-count behavior, and zero-delay GIF behavior.
- Playback phase policy when a resource is shared by several nodes.
- Visibility policy: pause, retain phase, or continue playback when no visible
  node references a resource.
- Exact CLP binary-upload message and resource-handle names.

### Acceptance criteria

- A valid GIF animates in a scene without client-driven frame swapping.
- Static textures and existing image nodes retain their behavior.
- Invalid, oversized, and malformed animation data fails safely per session.
- Resource cleanup releases all decoded and GPU state on node/session removal.
- Timing tests use a deterministic clock; manual playback is verified on Linux
  and Windows.

### Accepted decisions

_None yet._

### Validation record

_Not started._

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
