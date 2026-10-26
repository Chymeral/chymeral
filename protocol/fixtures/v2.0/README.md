# Archived CLP v2.0 fixture corpus

`fixtures.json` is an archived, machine-readable compatibility-regression
corpus. The active public conformance reference is the
[CLP v2.4 fixture corpus](../v2.4/README.md). This retained snapshot gives SDK
authors and reviewers concrete framed-message examples without making any
language client or the Chymeral host implementation a dependency of another
repository.

The normative contract remains the core repository's
[`Main_Manual.md`](../../../Main_Manual.md). This corpus deliberately is not an
IDL, a schema generator, or a package manifest. CLP also has lifecycle,
capability-negotiation, resource-ownership, binary-transfer, and delivery
semantics that cannot be established by a JSON shape alone.

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
Likewise, `session-1`, `node-N`, `image-N`, and `fixture-*` request IDs
are illustrative values. A conforming client must treat session and resource
handles as opaque, and must not depend on those example strings or numeric
suffixes.

## What it covers

- Version/capability handshake, subscriptions, resize, timing, and clean
  session closure.
- Retained scene creation, intrinsic text, atomic mutation, overlap events and
  queries, and cleanup.
- Ordered RGBA8 and GIF binary uploads, alpha contact, external-drop targeting,
  and resource destruction.
- Delivery-gap recovery data, stable command-error codes, and fault closure.

The corpus uses representative host results and notifications. It does not
replace a live conformance run: an SDK must still prove that it connects to a
real Chymeral endpoint and handles the capabilities actually granted by that
host.

## Using it in an SDK

1. Review and pin a Chymeral source revision or release containing this
   directory. Copy `fixtures.json` and `SHA256SUMS` together into the SDK;
   do not fetch either one during ordinary CI.
2. Make a public-API test replay every vector against that SDK's own fake host.
   It must compare outgoing JSON and binary frames, then verify incoming
   lifecycle, results, notifications, errors, and closure.
3. Verify the vendored SHA-256 in that test. Normalize only generated request
   IDs when comparing transcripts; resource handles must flow through the
   client as opaque values.
4. Keep independent focused tests and a live Chymeral conformance executable.
   The fixture corpus complements both.

## Archived compatibility matrix

Both official SDK repositories vendor the exact snapshot in this directory and
run its replay test without checking out this repository or using the network:

| SDK | Vendored snapshot test | Automated matrix |
| --- | --- | --- |
| [Rust SDK](https://github.com/Chymeral/chymeral-rust-sdk) | `cargo test --locked --test fixture_corpus` | Ubuntu and Windows on Rust 1.85 and stable |
| [Node SDK](https://github.com/Chymeral/chymeral-node-sdk) | `npm test` | Ubuntu and Windows on Node.js 22 and 24 |

When CLP changes, update the relevant vector(s), the in-tree fixture validation
test, `SHA256SUMS`, protocol documentation, host tests, and every affected SDK
snapshot in the same review. A backwards-incompatible protocol revision receives
a new versioned directory rather than rewriting this v2.0 corpus.
