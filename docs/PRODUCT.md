# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Astro + Starlight, static output, GitHub Pages deployment target.

## Users

Primary users are developers and maintainers who need to turn real application behavior into regression tests without instrumenting application code. Secondary users are contributors who need to understand Chronicle's crate boundaries, capture pipeline, safety model, and validation workflows.

## Product Purpose

Chronicle records real application traffic and turns it into deterministic, replayable regression-test evidence. The site must help a developer decide whether Chronicle fits, install the current Linux binary, run a first recording, understand the capture-to-replay pipeline, and operate it without unsafe assumptions.

## Positioning

Chronicle observes a supervised command, process, or cgroup with eBPF, writes evidence to a crash-recoverable local WAL before interpretation, reconstructs a protocol-independent canonical session, and replays only against explicitly authorized loopback targets. Its differentiator is this durable, storage-independent path from real behavior to portable replayable evidence with minimal application integration.

## Operating Context

Users work from a shell and repository. The current public CLI is intent-oriented: `record`, `replay`, `list`, `inspect`, and `doctor`. They may capture a supervised command, an existing process, or a cgroup on supported Linux; inspect local recordings; replay into a fresh supervised application or an explicitly authorized loopback target; and consult operations, architecture, safety, and release material. The website is built as static files for GitHub Pages and must remain usable without client-side application state.

## Capabilities and Constraints

- Current end-to-end functional protocol: bounded plaintext HTTP/1.1 only.
- Live capture: Linux 6.1+, cgroup v2, BTF, little-endian x86_64/aarch64, and eBPF capabilities; no application instrumentation.
- Recording lifetime is optional at the public surface; omitted `--duration` has no whole-run deadline. Epoch and segment WAL limits remain bounded, and a parent recording may own multiple immutable epoch sessions.
- WAL v1 is segmented, append-only, group-committed, checksum-validated, and recovery-authoritative through in-WAL commit markers.
- ETL reconstructs bounded sessions and atomically publishes one canonical session per finalized epoch to local filesystem storage; a WAL epoch boundary is not a protocol reconstruction boundary.
- Replay is dry-run by default, denies effects until authorized, requires loopback target mapping, and never falls back to the recorded production destination.
- Public data is local filesystem storage today. PostgreSQL, S3-compatible persistence, TLS decryption, HTTP/2+, additional protocols, encryption at rest, comprehensive redaction, Docker packaging, and Kubernetes packaging are not implemented.
- Canonical English is the source for localized site content. Traditional Chinese (zh-TW) and Japanese (ja) are supported locales; technical terms must remain consistent through a glossary.
- Documentation has a current/Latest surface and an archived 0.1 surface; versioning must be isolated so future published versions can be added without rewriting the site.

## Brand Commitments

The product name is Chronicle. Existing repository banner artwork lives at `docs/branding/banner.png`, but it is reference material rather than a required hero asset. Product voice is precise, quiet, technical, durable, trustworthy, minimal, and developer-focused. The website must avoid generic AI-SaaS patterns and must not fabricate customers, metrics, testimonials, or capabilities.

## Evidence on Hand

- `README.md`: current product summary, installation, quick start, capabilities, safety, limitations, CLI table, and architecture overview.
- `docs/operations.md`: operational bounds, recording, recovery, ETL, inspect, replay, doctor, and migration details.
- `docs/architecture.md`, `docs/architecture/crate-boundaries.md`, `docs/canonical-model.md`, `docs/replay-safety.md`, and `docs/wal-format.md`: canonical architecture and safety facts.
- `crates/chronicle-cli/src/main.rs`: exact public command names and flags.
- `install.sh` and `.github/workflows/release.yml`: release installation and artifact contracts.
- `docs/branding/banner.png`: existing project banner.
- No customer proof, public usage metrics, performance benchmarks, or deployment integrations are available; future pages must not invent them.

## Product Principles

1. Show real behavior, not synthetic promises.
2. Make durability and recovery legible without exposing unnecessary internals.
3. Keep replay authorization explicit and safety-first.
4. Prefer portable canonical evidence over capture or storage coupling.
5. Keep production integration and website runtime cost small.

## Accessibility & Inclusion

Use semantic HTML, keyboard-visible focus, WCAG AA contrast targets, reduced-motion support, responsive layouts at narrow widths, accessible locale/version controls, and typography that remains readable in Latin, Traditional Chinese, and Japanese.
