# Contributing to Chymeral

Thank you for improving Chymeral. This repository contains the terminal
application and local graphics runtime. Its supported application boundary is
the language-neutral Chymeral Link Protocol (CLP); it is not a public Rust
framework. Rust applications should use the separate
[`chymeral-rust-sdk`](https://github.com/Chymeral/chymeral-rust-sdk) or speak
CLP directly.

## Development setup

Chymeral supports Linux and Windows. The repository pins Rust `1.90.0` through
`rust-toolchain.toml`; install it with Rustup if it is not already available:

```text
rustup toolchain install 1.90.0 --component rustfmt --component clippy
```

Build and run the terminal with:

```text
cargo run --locked --release
```

Use `chymeral.toml.example` for visual configuration. Do not commit personal
configuration files, generated binaries, screenshots, or `target/` contents.

## Required local checks

Run these from the repository root before opening a pull request:

```text
cargo fmt --check
cargo test --locked --all-targets -- --test-threads=1
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release --all-targets
```

Also build the documentation with warnings denied. In a Unix shell, run
`RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps`; in PowerShell, run
`$env:RUSTDOCFLAGS = "-D warnings"; cargo doc --locked --no-deps`.

Changes affecting input, rendering, the PTY, desktop integration, or CLP also
need manual validation on the supported platform(s). When a change can differ
between Linux and Windows, state which platforms you exercised in the pull
request.

## Scope and review

Keep changes focused, formatted, and covered by the closest appropriate test.
Prefer a small follow-up change over opportunistic refactors. The large host
modules deliberately keep related state and invariants together; do not move
code solely to reduce line count without preserving that cohesion.

Treat CLP as a public contract. A CLP change must update the protocol types,
host behavior, wire tests, `Main_Manual.md`, the compatibility record, and the
reference SDK when applicable. Do not add silent protocol aliases or make Rust
a privileged runtime path. Discuss breaking contract changes before coding
them.

The core Rust modules are shared executable internals, not a semver-stable API.
Avoid depending on them from external projects. Security issues belong in the
[security policy](SECURITY.md), not in public issues.
