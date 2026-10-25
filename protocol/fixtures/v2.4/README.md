# CLP v2.4 fixture corpus

`fixtures.json` is the portable, machine-readable regression corpus for the
CLP v2.4 `scene_batch` extension. It supplements, but never rewrites, the
frozen [v2.0](../v2.0/README.md) and [v2.1](../v2.1/README.md) corpora.

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
Likewise, `session-N`, `node-N`, and batch aliases are illustrative values.
Session and resource identifiers remain opaque to a host except for documented
validation and correlation rules.

## What it covers

- The CLP v2.4 handshake and explicit `scene_batch` capability negotiation.
- Ordered `create_node`, `set_node`, and `remove_node` operations carried by
  one bounded, atomic command result.
- Batch-local aliases for a parent created before its text child.
- Returned opaque node handles and text metrics, aligned operation outcomes,
  omitted temporary aliases, and clean session closure.

`scene_batch` uses the reliable command lane. Once accepted, Chymeral commits
the structural transition as one retained-scene state and requests one redraw;
it does not report physical scanout timing. If the capability is absent, an SDK
must report that condition rather than silently replaying the batch as visible
sequential commands. CLP v2.4 hosts continue to accept v2.0 and v2.1 clients;
those clients cannot obtain `scene_batch` and retain their established command
paths.

## Using it in an SDK

1. Vendor `fixtures.json` and `SHA256SUMS` together; do not fetch them during
   ordinary CI.
2. Replay every vector through the SDK's public API and its own fake host.
   Compare outgoing frames and verify lifecycle, command results, errors, and
   closure.
3. Verify the vendored SHA-256. Normalize only generated command request IDs;
   aliases and fixture request IDs in this corpus remain exact.
4. Keep focused protocol tests and a live Chymeral conformance executable. The
   corpus complements both.

Both official SDKs vendor this exact snapshot and test it independently. This
corpus is prepared for the Chymeral `1.7.0` release as the portable conformance
reference for CLP v2.4.
