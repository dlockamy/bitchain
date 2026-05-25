# bitchain — Agent Context
# clusterzer0/bitchain
#
# Paste this file into your AI's system prompt to work on this repo.
# ─────────────────────────────────────────────────────────────────────────────

You are working on **bitchain** — a Rust CLI for content-addressed binary
versioning, created by Douglas Lockamy (DJ) at Lockamy Studios.

Be precise about the data model. Content-addressing is the core invariant —
a block's hash is its identity. Never suggest changes that compromise that.

---

## What bitchain is

A lightweight virtual filesystem that uses the internet as block storage.
Files are split into SHA-256 addressed blocks and stored in S3 (or compatible).
A JSON manifest records the block list and metadata for each version.

Think: a minimal, self-hosted Git LFS for binary artifacts, with a clean CLI.

**Primary use case:** versioning large binary artifacts (OS images, compiled
firmware, game builds) where Git is too slow and S3 alone has no version semantics.

---

## Core data model

```
Block:
  hash: SHA-256 of content (hex string) — the block's identity
  size: byte count
  data: raw bytes (stored in S3 at key = hash)

Manifest:
  id:      UUID
  name:    human-readable artifact name
  version: semver string
  blocks:  [{ hash, size, offset }]  — ordered list
  created: ISO 8601 timestamp
  meta:    arbitrary JSON
```

A manifest reconstructs the original file by fetching blocks in order and
concatenating. Block hashes are verified on fetch. Deduplication is automatic —
identical blocks across versions share storage.

---

## Stack

- Language: Rust (edition 2021)
- CLI framework: `clap` v4 with derive macros
- Async runtime: `tokio` (full features)
- Serialization: `serde` + `serde_json`
- Hashing: `sha2` v0.10
- S3: `aws-sdk-s3` v1 + `aws-config` v1
- HTTP: `reqwest` v0.11 with JSON feature

---

## Architecture (planned)

```
bitchain/
├── src/
│   ├── main.rs          — CLI entry, clap subcommands
│   ├── block.rs         — block splitting, hashing, storage
│   ├── manifest.rs      — manifest creation, serialisation, lookup
│   ├── store/
│   │   ├── mod.rs       — Store trait
│   │   ├── s3.rs        — S3 backend
│   │   └── local.rs     — local filesystem backend (dev/test)
│   └── config.rs        — config file, env vars, AWS region
├── Cargo.toml
└── Jenkinsfile          — CI: fmt + clippy + test + publish to Nexus cargo-hosted
```

---

## CLI commands (planned)

```
bitchain push  <file> [--name <name>] [--version <ver>]  — split, hash, upload, emit manifest
bitchain pull  <manifest-id>                             — fetch blocks, reassemble file
bitchain ls    [--name <name>]                           — list manifests
bitchain diff  <manifest-a> <manifest-b>                 — block-level diff between versions
bitchain gc    [--dry-run]                               — remove unreferenced blocks
```

---

## Refraction relationship

**Refraction** (`clusterzer0/refraction`) is the server-side companion to bitchain:
- bitchain is the CLI tool (local)
- Refraction is the management API (self-hosted, enterprise)

The Refraction API endpoints bitchain should support:
- `GET /block/{hash}` — fetch a block
- `POST /manifest` — store a manifest
- `GET /manifest/{id}` — retrieve a manifest

When adding Refraction backend support, target these endpoints exactly.

---

## CI/CD

Jenkins pipeline: Pre-flight → `cargo fmt --check` + `cargo clippy` + `cargo test`
+ `cargo build --release` → publish to `nexus.softsurve.com/repository/cargo-hosted/`.

Jenkins credential: `nexus-credentials`.
Cargo registry config at deploy time — not committed to repo.

---

## Open work

- [ ] Implement `block.rs` — splitting, SHA-256, upload/fetch
- [ ] Implement `manifest.rs` — create, serialise, store, retrieve
- [ ] Implement S3 store backend
- [ ] Implement local filesystem store (for tests / dev)
- [ ] Wire up CLI subcommands
- [ ] Add Refraction HTTP backend option
- [ ] Write integration tests against local store

---

## Jira

Project: **LS** — fairmerce.atlassian.net (new work)
Legacy: KAN-80 (bitchain CLI epic), KAN-81 (bitchain-studio epic).
