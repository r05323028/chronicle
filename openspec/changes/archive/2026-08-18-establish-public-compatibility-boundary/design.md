## Context

See `proposal.md` for release-readiness motivation. The repository already has domain-specific v1 requirements and a release workflow with dry-run preparation, but its central versioning capability still describes a pre-release freeze gate. Current validation also checks that release snapshot paths exist without checking whether snapshot content claims a future release.

## Goals / Non-Goals

**Goals:**

- Create one public compatibility authority for 0.1 while preserving domain-specific format details in WAL, capture, canonical, storage, CLI, and replay specs.
- Make released installation documentation and release workflow checks reflect actual shipped behavior.
- Preserve fail-closed replay and least-privilege publication boundaries.
- Produce fresh, fingerprinted privileged evidence for the release candidate rather than relying on stale proof.

**Non-Goals:**

- No new public command, output feature, storage backend, migration adapter, package channel, Docker/Kubernetes packaging, or crates.io publication.
- No permanent freeze for private eBPF ABI, transient state, advisory catalog/metadata, or every internal file.
- No dependency-tool installation or cargo-deny bootstrap in release validation.

## Decisions

### D1: Replace, do not layer, pre-release policy

Add `public-compatibility-boundary` as the single cross-domain policy. Remove all requirements from `mvp-schema-versioning` through the OpenSpec delta, then update active dependent text that names the old policy. Domain-specific specifications remain authoritative for byte layouts and safety mechanics; the new capability defines compatibility scope and change process only.

Alternative rejected: keeping both policies would let contributors choose between a pre-release exception and the public contract, creating duplicate authority.

### D2: Freeze contract surfaces, not implementation details

The frozen set is limited to persisted/versioned WAL v1, intentionally persisted Capture Event v1, Canonical Session v1, Session Manifest v1, replay authorization, documented public command behavior, and documented stable JSON. Private eBPF ABI, hidden `internal` commands, advisory metadata, and transient state remain outside the cross-domain freeze unless their owning specification makes them public.

Alternative rejected: freezing every file or Rust type would block safe implementation fixes without improving user compatibility.

### D3: Use semantic release-state predicates

`verify-release-docs.mjs` keeps its existing version-directory/sidebar checks and adds a small locale-keyed list of contradiction predicates for the requested snapshot. It reads Markdown recursively, reports exact file paths and matched predicate names, and does not compare prose, headings, or translations byte-for-byte.

Alternative rejected: a full prose snapshot test would be brittle under legitimate editorial and translation changes; directory existence alone is too weak.

### D4: Pin action by verified commit identity

Resolve `v2.27.0` once to commit `0698813906fb1f23425bd742510e3080d624840d` and put that full SHA in the workflow. The portable test accepts only exactly 40 hexadecimal characters and separately asserts the intended SHA, preventing a semantic version tag from being misclassified as immutable.

Alternative rejected: a tag or abbreviated SHA can move or be ambiguous, which is unacceptable for a step receiving `OPENCODE_GO_API_KEY`.

### D5: Keep release publication boundary unchanged

Retain `workflow_dispatch` as preparation-only, the existing `publish` condition, `contents: write` only on `publish`, explicit `GH_TOKEN`/`GH_REPO`, and secret injection only in the Pi step. The change touches only action identity and tests around that boundary.

### D6: Treat support evidence as compatibility evidence

Run the final release profile with `--no-reuse` from the candidate checkout and retain the existing compact/fingerprinted evidence. Record environment identity separately for x86_64 and aarch64/arm64; do not convert a different architecture or kernel into proof for an untested claim. If a required environment cannot be obtained, the final support text must be narrowed rather than labeled verified.

### D7: Do not wire unavailable dependency tooling

`deny.toml` is already selected by validation ownership, but `cargo-deny` is not installed and no CI job currently invokes it. Do not add network installation or a fragile optional command in this release change. Revisit when the repository has a pinned, portable cargo-deny toolchain and an explicit CI owner.

## Risks / Trade-offs

- **Release snapshot guard false positives** → Keep predicates limited to explicit unreleased-state claims and cover all three locale roots; allow ordinary historical references outside the requested snapshot.
- **Action SHA mismatch** → Preserve the verified tag-to-SHA lookup in the change record and assert the exact intended SHA in the portable test.
- **Compatibility scope too broad** → Exclude private/advisory surfaces explicitly and keep format details in owning specs.
- **Support matrix evidence unavailable on the current host** → Attempt an x86_64 Linux Multipass environment; if unavailable, retain the blocker and narrow documentation rather than reusing aarch64 proof.
- **Installer preflight portability** → Check only commonly available commands already used, keep POSIX `sh`, and exercise existing fixture tests on macOS/Linux.

## Migration Plan

1. Implement and validate the compatibility/spec/documentation/tooling changes on the candidate checkout.
2. Run `graphify update .` after source changes, then strict OpenSpec, focused tests, fast validation, and fresh release acceptance.
3. Verify the manual release preparation path and inspect archives/checksums/release notes without creating a tag or release.
4. Sync the new capability and deltas into active specs, verify the change, and archive it. Rename any remaining active pre-release requirement headings after sync so only archived history retains the retired name.
5. Re-run final checks from the unchanged candidate commit before recommending a tag.
