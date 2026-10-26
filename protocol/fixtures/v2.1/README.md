# Archived CLP v2.1 fixture corpus

`fixtures.json` is an archived, machine-readable compatibility-regression
corpus. The active public conformance reference is the
[CLP v2.4 fixture corpus](../v2.4/README.md). This retained snapshot
supplements, but never rewrites, the frozen [v2.0 corpus](../v2.0/README.md).

The normative contract remains the core repository's
[`Main_Manual.md`](../../../Main_Manual.md). This corpus is not an IDL, schema
generator, or package manifest. Lifecycle, capability negotiation, ownership,
coalescing, and frame-boundary semantics still apply even where they do not fit
inside a single JSON message.

`SHA256SUMS` fixes the exact corpus bytes for downstream snapshots. It detects
accidental drift after an SDK vendors the file; it is an integrity check, not a
release signature. The official repositories force LF checkout for the
byte-checked files so Windows does not alter their hash.

## Format

The corpus has a format version (`fixture_version`) and declares the CLP version
it represents. Each vector is an ordered transcript of frames with a direction:

| Field | Meaning |
| --- | --- |
| `direction` | `client_to_host` or `host_to_client`. |
| `kind` | `json` for a UTF-8 JSON payload, or `binary` for raw bytes. |
| `message` | The JSON object for a `json` frame. |
| `bytes_hex` | The raw bytes for a `binary` frame, encoded as lowercase hexadecimal only for this fixture file. |

On the wire, every frame starts with a four-byte unsigned big-endian length.
That length includes the one-byte discriminator: ASCII `J` for JSON and `B`
for binary. `bytes_hex` is **not** a CLP wire field and must never be sent as
JSON or converted to base64 by a client.

The token in the lifecycle vector is a literal sentinel, not a credential.
Likewise, `session-N`, `node-N`, `image-N`, and `fixture-presentation-N` are
illustrative values. Session, resource, and presentation identifiers are opaque
to a host except for the documented validation and correlation rules.

## What it covers

- The CLP v2.1 handshake and the opt-in `presentation` and
  `window_metadata` capabilities.
- A `present_scene` packet containing two geometry changes committed as one
  frame-boundary visual transition.
- An asynchronous `presentation_status` confirmation and a rejected text-bounds
  packet with its stable error code.
- Clean closure after presentation traffic without a command response for either
  packet.
- A session-owned application title plus a static RGBA8 image handle, including
  an icon-support context, title-only replacement that preserves the icon, and
  explicit `null` clearing of both fields.

`present_scene` is a latest-state lane. The host may replace an unpresented
packet from the same session, and status notifications may be coalesced under
backpressure. An application must treat a status as diagnostic correlation, not
as a reliable per-sample completion queue. Structural work remains on the
ordered command lane.

## Using it in an SDK

1. Vendor `fixtures.json` and `SHA256SUMS` together; do not fetch them during
   ordinary CI.
2. Replay every vector through the SDK's public API and its own fake host.
   Compare outgoing frames and verify lifecycle, command results, presentation
   statuses, errors, and closure.
3. Verify the vendored SHA-256. Normalize only generated command request IDs;
presentation IDs in this corpus are application supplied and must remain exact.
4. Keep focused protocol tests and a live Chymeral conformance executable. The
   corpus complements both.

Both official SDKs vendor this exact snapshot and test it independently. This
archived corpus remains for compatibility regression; the v2.4 corpus is the
portable public conformance reference for the complete current contract.
