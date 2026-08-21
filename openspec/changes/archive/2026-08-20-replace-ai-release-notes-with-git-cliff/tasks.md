## 1. OpenSpec and contract

- [x] 1.1 Add focused proposal, design, task list, and release-packaging delta.
- [x] 1.2 Update active release-packaging specification without changing archived historical changes.

## 2. Deterministic generator

- [x] 2.1 Add repository-managed `cliff.toml` with strict Chronicle Conventional Commit parsing, product-only groups, breaking protection, stable ordering, and GitHub presentation enrichment.
- [x] 2.2 Add and test explicit semantic release-range resolver with frozen-SHA and intended-tag checks.
- [x] 2.3 Replace release-note workflow steps with pinned git-cliff 2.13.1 installation, read-only GitHub metadata enrichment, and `release-notes.md` artifact generation.
- [x] 2.4 Remove obsolete release-note prompt, Pi/OpenCode provider setup, model configuration, and API-key plumbing.

## 3. Tests and documentation

- [x] 3.1 Replace Pi/OpenCode assertions with git-cliff workflow/config/range/retry/prepare/publication and GitHub-enrichment assertions.
- [x] 3.2 Add temporary-repository fixture tests for product grouping, excluded types, skipped-type breaking changes, breaking footers, metadata conditionals, range exclusion, determinism, and invalid commits.
- [x] 3.3 Update `RELEASING.md` to document squash history → git-cliff → GitHub-enriched `release-notes.md`.

## 4. Validation

- [x] 4.1 Run focused workflow, range, and git-cliff tests plus representative local product-history rendering without tag mutation.
- [x] 4.2 Run strict OpenSpec validation and `./scripts/validate.sh fast`.
- [x] 4.3 Run `graphify update .` after source changes and review final diff/stale references.
