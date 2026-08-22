#!/usr/bin/env python3
"""Small, dependency-free helpers for layered Chronicle validation."""

from __future__ import annotations

import argparse
import collections
import datetime as dt
import fnmatch
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 fallback is documented.
    tomllib = None


def run(root: Path, *args: str) -> str:
    try:
        return subprocess.check_output(
            args, cwd=root, text=True, stderr=subprocess.DEVNULL, timeout=30
        ).strip()
    except (OSError, subprocess.SubprocessError):
        return "not_checked"


def config(root: Path, path: Path | None = None) -> dict[str, Any]:
    if tomllib is None:
        raise RuntimeError("Python 3.11+ is required (tomllib missing)")
    with (path or root / "validation/groups.toml").open("rb") as handle:
        return tomllib.load(handle)


def changed_paths(root: Path, changed_since: str | None) -> list[str]:
    if changed_since:
        args = ("git", "diff", "--name-only", changed_since)
        paths = (
            run(root, *args).splitlines() if run(root, *args) != "not_checked" else []
        )
    else:
        paths = []
        for args in (
            ("git", "diff", "--name-only", "HEAD"),
            ("git", "diff", "--cached", "--name-only"),
        ):
            value = run(root, *args)
            if value != "not_checked":
                paths.extend(value.splitlines())
    untracked = run(root, "git", "ls-files", "--others", "--exclude-standard")
    if untracked != "not_checked":
        paths.extend(untracked.splitlines())
    return sorted(set(path for path in paths if path))


def matches(path: str, pattern: str) -> bool:
    path = path.removeprefix("./")
    return fnmatch.fnmatchcase(path, pattern) or Path(path).match(pattern)


def select(root: Path, paths: list[str], cfg: dict[str, Any]) -> dict[str, Any]:
    groups = cfg.get("groups", {})
    selected: list[str] = []
    decisions: dict[str, dict[str, Any]] = {}
    matched_paths: set[str] = set()
    for name in sorted(groups):
        group = groups[name]
        hits = [
            path
            for path in paths
            if any(matches(path, pattern) for pattern in group.get("paths", []))
        ]
        matched_paths.update(hits)
        if hits:
            selected.append(name)
            decisions[name] = {
                "selected": True,
                "paths": hits,
                "reason": f"matched {', '.join(hits)}",
            }
        else:
            decisions[name] = {
                "selected": False,
                "paths": [],
                "reason": "no changed path matches group ownership",
            }
    unknown_paths = sorted(set(paths) - matched_paths)
    if unknown_paths:
        selected = sorted(groups)
        for name in selected:
            decisions[name] = {
                "selected": True,
                "paths": decisions[name]["paths"],
                "reason": f"unknown path requires conservative full validation: {', '.join(unknown_paths)}",
            }
    # Catalog-aware annotation (task 9.1): report functional layer, environment,
    # owner, and gate reason for each selected group's gate-relevant tests, plus
    # a no-coverage-loss marker. Missing catalog degrades to group-level report.
    layers: set[str] = set()
    environments: set[str] = set()
    owners: set[str] = set()
    catalog_path = root / "validation/test-architecture/test-catalog.toml"
    catalog = None
    if tomllib is not None and catalog_path.is_file():
        with catalog_path.open("rb") as handle:
            catalog = tomllib.load(handle)
    if catalog is not None:
        tests = catalog.get("test", [])
        for name in selected:
            group_gates = set(groups[name].get("gates", []))
            relevant = [
                test
                for test in tests
                if not group_gates or (set(test.get("gates", [])) & group_gates)
            ]
            decisions[name]["tests"] = [
                {
                    "id": test["id"],
                    "layer": test.get("layer"),
                    "environment": test.get("environment"),
                    "owner": test.get("owner"),
                }
                for test in relevant
            ]
            for test in relevant:
                if test.get("layer"):
                    layers.add(test["layer"])
                if test.get("environment"):
                    environments.add(test["environment"])
                if test.get("owner"):
                    owners.add(test["owner"])
        coverage = "catalog obligations verified: required selectors reference classified tests"
    else:
        coverage = "no coverage-loss decision available: catalog not present"
    return {
        "changed_paths": paths,
        "unknown_paths": unknown_paths,
        "selected": selected,
        "decisions": decisions,
        "layers_covered": sorted(layers),
        "environments_covered": sorted(environments),
        "owners_covered": sorted(owners),
        "coverage": coverage,
    }


def _cargo_metadata(root: Path) -> dict[str, Any]:
    """Read bounded Cargo metadata once per architecture data source."""
    try:
        output = subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            cwd=root,
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=120,
        )
    except (OSError, subprocess.SubprocessError):
        raise RuntimeError("bounded cargo metadata failed; cannot check architecture")
    try:
        return json.loads(output)
    except (ValueError, TypeError) as exc:
        raise RuntimeError(
            "cargo metadata returned malformed JSON; cannot check architecture"
        ) from exc


def workspace_edges(root: Path) -> list[dict[str, Any]]:
    """Capture Chronicle workspace path dependencies from bounded Cargo metadata."""
    value = _cargo_metadata(root)
    workspace_members = set(value.get("workspace_members", []))
    packages = {pkg["id"]: pkg for pkg in value.get("packages", [])}
    ws = {
        pkg["name"]: pkg
        for pkg in packages.values()
        if pkg["id"] in workspace_members and pkg["name"].startswith("chronicle-")
    }
    # Resolve package identity by manifest directory so renames and local
    # aliases never change the evaluated target crate.
    by_manifest_dir = {
        str(Path(pkg["manifest_path"]).parent): pkg["name"] for pkg in ws.values()
    }
    edges: list[dict[str, Any]] = []
    for name in sorted(ws):
        for dep in sorted(ws[name].get("dependencies", []), key=lambda d: d["name"]):
            if not dep.get("path"):
                continue
            identity = by_manifest_dir.get(str(Path(dep["path"])))
            if identity is None or identity not in ws:
                continue
            edges.append(
                {
                    "source": name,
                    "target": identity,
                    "rename": dep.get("rename") or dep["name"],
                    "kind": dep.get("kind") or "normal",
                    "optional": bool(dep.get("optional")),
                    "target_condition": dep.get("target"),
                }
            )
    return edges


def external_dependencies(root: Path) -> list[dict[str, Any]]:
    """Capture direct non-workspace package dependencies by Cargo package identity."""
    value = _cargo_metadata(root)
    workspace_members = set(value.get("workspace_members", []))
    packages = {pkg["id"]: pkg for pkg in value.get("packages", [])}
    workspace = {
        pkg["name"]: pkg
        for pkg in packages.values()
        if pkg["id"] in workspace_members and pkg["name"].startswith("chronicle-")
    }
    by_manifest_dir = {
        str(Path(pkg["manifest_path"]).parent): pkg["name"]
        for pkg in workspace.values()
    }
    dependencies: list[dict[str, Any]] = []
    for source in sorted(workspace):
        for dep in sorted(
            workspace[source].get("dependencies", []), key=lambda d: d["name"]
        ):
            target = (
                by_manifest_dir.get(str(Path(dep["path"]))) if dep.get("path") else None
            )
            if target in workspace:
                continue
            dependencies.append(
                {
                    "source": source,
                    "package": dep["name"],
                    "rename": dep.get("rename") or dep["name"],
                    "kind": dep.get("kind") or "normal",
                    "optional": bool(dep.get("optional")),
                    "target_condition": dep.get("target"),
                }
            )
    return dependencies


def _distribution_closure_paths(
    roots: list[str], edges: list[dict[str, Any]]
) -> dict[str, tuple[list[str], str]]:
    """Map each workspace member reachable from a distribution root to its path.

    Reachability follows normal and build workspace edges only: those are the
    dependencies the default executable installs and links. Dev dependencies
    never link into an executable build graph and are deliberately excluded.
    Returns member -> (member names from root to member, final hop kind).
    """
    children: dict[str, list[tuple[str, str]]] = {}
    for edge in edges:
        if edge["kind"] in ("normal", "build"):
            children.setdefault(edge["source"], []).append(
                (edge["target"], edge["kind"])
            )
    reached: dict[str, tuple[list[str], str]] = {
        crate: ([crate], "root") for crate in roots
    }
    queue = collections.deque(roots)
    while queue:
        source = queue.popleft()
        path, _ = reached[source]
        for target, kind in sorted(children.get(source, [])):
            if target in reached:
                continue
            reached[target] = (path + [target], kind)
            queue.append(target)
    return reached


def _provider_dependency_detail(dependency: dict[str, Any]) -> str:
    """Suffix describing kind, rename, optionality, and target condition."""
    detail = f"[{dependency['kind']}]"
    if dependency["rename"] != dependency["package"]:
        detail += f" [renamed as {dependency['rename']}]"
    if dependency["optional"]:
        detail += " [optional]"
    if dependency["target_condition"]:
        detail += f" [{dependency['target_condition']}]"
    return detail


def _external_dependency_issues(
    root: Path,
    policy: dict[str, Any],
    members: set[str],
    edges: list[dict[str, Any]],
) -> list[str]:
    """Reject provider SDKs at core/domain boundaries and inside the default
    Chronicle distribution dependency closure."""
    protected = policy.get("core_domain_crates", [])
    roots = policy.get("default_distribution_roots", [])
    forbidden = policy.get("forbidden_packages", [])
    values = (protected, roots, forbidden)
    if not all(isinstance(value, list) for value in values):
        return [
            "external_dependencies must declare list-valued "
            "core_domain_crates, default_distribution_roots, and forbidden_packages"
        ]
    issues: list[str] = []
    for crate in protected:
        if crate not in members:
            issues.append(
                f"external dependency policy names unknown core/domain crate {crate!r}"
            )
    if not roots:
        issues.append(
            "external dependency policy must declare a non-empty "
            "default_distribution_roots list"
        )
    for crate in roots:
        if crate not in members:
            issues.append(
                f"external dependency policy names unknown default distribution "
                f"root {crate!r}"
            )
    if len(set(forbidden)) != len(forbidden):
        issues.append(
            "external dependency policy forbidden_packages contain duplicates"
        )
    protected_set = set(protected) & members
    known_roots = [crate for crate in roots if crate in members]
    closure = _distribution_closure_paths(known_roots, edges)
    denied = set(forbidden)
    for dependency in external_dependencies(root):
        if dependency["package"] not in denied:
            continue
        source = dependency["source"]
        if source in protected_set:
            issues.append(
                f"forbidden external provider dependency: "
                f"{source} -> {dependency['package']} "
                + _provider_dependency_detail(dependency)
            )
            continue
        if source in closure and dependency["kind"] != "dev":
            # Dev declarations never link into the shipped executable.
            path, _ = closure[source]
            lines = [path[0]] + [f"  -> {name}" for name in path[1:]]
            lines.append(f"  -> {dependency['package']}")
            lines[-1] += " " + _provider_dependency_detail(dependency)
            issues.append(
                "forbidden provider dependency in default Chronicle distribution:\n"
                + "\n".join(lines)
            )
    return issues


def architecture_check(root: Path, path: Path | None = None) -> dict[str, Any]:
    """Validate Chronicle workspace dependency direction against architecture policy."""
    if tomllib is None:
        raise RuntimeError("Python 3.11+ is required (tomllib missing)")
    policy_path = path or root / "validation/architecture.toml"
    with policy_path.open("rb") as handle:
        policy = tomllib.load(handle)
    if policy.get("version") != 1:
        raise RuntimeError(
            f"unsupported architecture policy version {policy.get('version')}"
        )
    members = policy.get("members", {}).get("members", [])
    kinds = ("normal", "dev", "build")
    tables = {kind: policy.get(kind, {}) for kind in kinds}
    issues: list[str] = []
    member_set = set(members)
    if len(member_set) != len(members):
        issues.append("policy members contain duplicates")
    for kind in kinds:
        for source, targets in tables[kind].items():
            if source not in member_set:
                issues.append(
                    f"policy {kind} allowlist names unknown source crate {source!r}"
                )
            for target in targets:
                if target not in member_set:
                    issues.append(
                        f"policy {kind} allowlist names unknown target crate {target!r}"
                    )
    for member in sorted(member_set):
        for kind in kinds:
            if member not in tables[kind]:
                issues.append(f"policy missing {kind} allowlist for member {member!r}")
    forbids = policy.get("critical_forbids", {})
    forbid_keys = {
        "dependency_on_cli",
        "session_to_wal",
        "protocol_to_builtins",
        "common_upward",
        "cli_non_application",
    }
    if not isinstance(forbids, dict) or not forbid_keys.issubset(forbids):
        issues.append(
            "critical_forbids must declare every named forbid: "
            + ", ".join(sorted(forbid_keys))
        )
    for key in sorted(forbid_keys):
        if not isinstance(forbids.get(key), bool):
            issues.append(f"critical_forbids.{key} must be a boolean")
    edges = workspace_edges(root)

    def edge_key(edge):
        return (edge["source"], edge["target"], edge["kind"])

    for edge in sorted(edges, key=edge_key):
        source, target, kind = edge["source"], edge["target"], edge["kind"]
        if source not in member_set:
            issues.append(
                f"workspace crate {source!r} has no architecture policy member entry"
            )
            continue
        if target not in member_set:
            issues.append(
                f"workspace edge {source} -> {target} targets a crate with no policy entry"
            )
            continue
        if forbids.get("dependency_on_cli") and target == "chronicle-cli":
            issues.append(
                f"forbidden dependency on chronicle-cli: {source} -> {target} [{kind}]"
            )
            continue
        if (
            forbids.get("session_to_wal")
            and source == "chronicle-session"
            and target == "chronicle-wal"
        ):
            issues.append(
                f"forbidden session-to-wal coupling: {source} -> {target} [{kind}]"
            )
        if (
            forbids.get("protocol_to_builtins")
            and source == "chronicle-protocol"
            and target == "chronicle-protocol-builtins"
        ):
            issues.append(
                f"forbidden protocol-to-builtins coupling: {source} -> {target} [{kind}]"
            )
        if (
            forbids.get("common_upward")
            and source == "chronicle-common"
            and kind != "build"
        ):
            issues.append(
                f"forbidden common upward dependency: {source} -> {target} [{kind}]"
            )
        if (
            forbids.get("cli_non_application")
            and source == "chronicle-cli"
            and target != "chronicle-application"
        ):
            issues.append(
                f"forbidden cli non-application edge: {source} -> {target} [{kind}]"
            )
        allow = tables[kind].get(source, [])
        if target not in allow:
            detail = f"{source} -> {target} [{kind}]"
            if edge["optional"]:
                detail += " [optional]"
            if edge["target_condition"]:
                detail += f" [{edge['target_condition']}]"
            issues.append(f"unclassified or forbidden workspace edge: {detail}")
    cycles = find_cycles(edges)
    if cycles:
        issues.append(f"dependency cycle: {' -> '.join(cycles[0])}")
    external_policy = policy.get("external_dependencies", {})
    external_issues = (
        _external_dependency_issues(root, external_policy, member_set, edges)
        if external_policy
        else []
    )
    issues.extend(external_issues)
    semantic = policy.get("semantic", {})
    semantic_issues = semantic_check(root, semantic) if semantic else []
    issues.extend(semantic_issues)
    return {
        "policy": str(policy_path.relative_to(root)),
        "members": sorted(member_set),
        "edges": edges,
        "normal_edges": sum(1 for e in edges if e["kind"] == "normal"),
        "dev_edges": sum(1 for e in edges if e["kind"] == "dev"),
        "build_edges": sum(1 for e in edges if e["kind"] == "build"),
        "cyclic": bool(cycles),
        "semantic": bool(semantic),
        "external_dependencies": bool(external_policy),
        "issues": issues,
    }


SEMANTIC_INVARIANT = (
    "semantic-boundary violation: outer adapters operate only on "
    "application-owned contracts"
)


def _semantic_statement(path: Path, lines: list[str], start: int) -> tuple[str, int]:
    """Join a possibly multiline Rust statement starting at `start` into one string.

    Returns (joined_statement, next_index_after_statement).
    """
    depth = 0
    parts: list[str] = []
    i = start
    while i < len(lines):
        parts.append(lines[i])
        depth += lines[i].count("{") - lines[i].count("}")
        if depth <= 0 and lines[i].rstrip().endswith(";"):
            break
        i += 1
    return "\n".join(parts), i + 1


def _semantic_re_export_issues(root: Path, semantic: dict[str, Any]) -> list[str]:
    """Application must not re-export lower-layer vocabulary (invariant S2)."""
    forbidden = set(semantic.get("forbidden_re_export_sources", []))
    allowlist = set(semantic.get("allowed_re_exports", []))
    app_dir = root / "crates/chronicle-application/src"
    if not app_dir.is_dir():
        return []
    issues: list[str] = []
    use_re = re.compile(r"pub\s+use\s+([a-z0-9_-]+)\s*(?:::|;|$)")
    for path in sorted(app_dir.rglob("*.rs")):
        lines = path.read_text(errors="replace").splitlines()
        i = 0
        while i < len(lines):
            match = use_re.match(lines[i].strip())
            if not match:
                i += 1
                continue
            crate = match.group(1).replace("_", "-")
            if crate not in forbidden:
                i += 1
                continue
            statement, nxt = _semantic_statement(path, lines, i)
            if "::*" in statement:
                symbols = ["*"]
            elif "{" not in statement:
                symbols = ["*"]  # bare `pub use crate;` re-exports the crate
            else:
                body = statement[statement.find("{") + 1 : statement.rfind("}")]
                symbols = [s.strip() for s in body.split(",") if s.strip()]
            if "*" in symbols:
                bad = ["*"]  # glob/bare crate re-exports are never allowlisted
            else:
                bad = [
                    symbol
                    for symbol in symbols
                    if f"{crate}::{symbol}" not in allowlist
                ]
            if bad:
                rel = str(path.relative_to(root))
                issues.append(
                    f"{SEMANTIC_INVARIANT}\n"
                    f"location: {rel}:{i + 1}\n"
                    f"rationale: application must not re-export lower-layer vocabulary as an "
                    f"escape hatch around the dependency policy; only reviewed "
                    f"allowed_re_exports entries may cross\n"
                    f"remediation: remove the re-export or add a reviewed "
                    f"allowed_re_exports entry (crate::Symbol) with rationale; see "
                    f"docs/architecture/crate-boundaries.md#semantic-boundaries"
                )
            i = nxt
    return issues


def _semantic_vocabulary_issues(root: Path, semantic: dict[str, Any]) -> list[str]:
    """Outer adapters must not name lower-layer vocabulary (invariant S1)."""
    forbidden = semantic.get("forbidden_outer_vocabulary", [])
    if not forbidden:
        return []
    issues: list[str] = []
    for base in (
        root / "crates/chronicle-cli/src",
        root / "crates/chronicle-cli/tests",
    ):
        if not base.is_dir():
            continue
        for path in sorted(base.rglob("*.rs")):
            lines = path.read_text(errors="replace").splitlines()
            for line_no, line in enumerate(lines, start=1):
                if line.lstrip().startswith("//"):
                    continue  # doc/comment mentions are not violations
                for symbol in forbidden:
                    if re.search(rf"\b{re.escape(symbol)}\b", line):
                        rel = str(path.relative_to(root))
                        issues.append(
                            f"{SEMANTIC_INVARIANT}\n"
                            f"location: {rel}:{line_no}\n"
                            f"rationale: replay policy and protocol/replay error taxonomies "
                            f"stay behind the application seam; CLI operates only on "
                            f"application-owned contracts\n"
                            f"remediation: use the application-owned request/result/exit-code "
                            f"API; see docs/architecture/crate-boundaries.md#semantic-boundaries"
                        )
    return issues


def _semantic_policy_issues(root: Path, semantic: dict[str, Any]) -> list[str]:
    """Every forbidden symbol must resolve to a forbidden re-export source."""
    forbidden = semantic.get("forbidden_outer_vocabulary", [])
    sources = semantic.get("forbidden_re_export_sources", [])
    issues: list[str] = []
    present = [crate for crate in sources if (root / "crates" / crate / "src").is_dir()]
    if not present:
        return issues  # tree too minimal to verify symbol resolution
    for symbol in forbidden:
        located = False
        for crate in present:
            crate_src = root / "crates" / crate / "src"
            if any(
                symbol in path.read_text(errors="replace")
                for path in crate_src.rglob("*.rs")
            ):
                located = True
                break
        if not located:
            issues.append(
                f"semantic policy: forbidden symbol {symbol!r} does not resolve to any "
                f"forbidden_re_export_sources crate source; fix the vocabulary list or the "
                f"crate list"
            )
    for entry in semantic.get("allowed_re_exports", []):
        if "::" not in entry:
            issues.append(
                f"semantic policy: allowlist entry {entry!r} must be crate::Symbol"
            )
    return issues


def semantic_check(root: Path, semantic: dict[str, Any]) -> list[str]:
    """Semantic/API boundary checks (S1, S2, policy self-consistency)."""
    return (
        _semantic_policy_issues(root, semantic)
        + _semantic_re_export_issues(root, semantic)
        + _semantic_vocabulary_issues(root, semantic)
    )


def find_cycles(edges: list[dict[str, Any]]) -> list[list[str]]:
    adjacency: dict[str, set[str]] = {}
    for edge in edges:
        adjacency.setdefault(edge["source"], set()).add(edge["target"])
    visiting: set[str] = set()
    done: set[str] = set()
    found: list[list[str]] = []

    def dfs(node: str, path: list[str]) -> None:
        if node in done:
            return
        if node in visiting:
            index = path.index(node)
            found.append(path[index:] + [node])
            return
        visiting.add(node)
        for neighbor in sorted(adjacency.get(node, ())):
            dfs(neighbor, path + [node])
        visiting.discard(node)
        done.add(node)

    for node in sorted(adjacency):
        dfs(node, [node])
    return found


def source_ownership(root: Path, cfg: dict[str, Any]) -> dict[str, Any]:
    """Ensure every production application source has a validation owner."""
    tracked_value = run(root, "git", "ls-files", "crates/chronicle-application/src")
    if tracked_value == "not_checked":
        tracked = [
            str(path.relative_to(root))
            for path in (root / "crates/chronicle-application/src").rglob("*.rs")
        ]
    else:
        tracked = tracked_value.splitlines()
    groups = cfg.get("groups", {})
    owners: dict[str, list[str]] = {}
    unowned: list[str] = []
    for name in tracked:
        if not name.endswith(".rs"):
            continue
        matches_for_file = [
            group_name
            for group_name, group in groups.items()
            if "recorder" in group.get("gates", [])
            and any(matches(name, pattern) for pattern in group.get("paths", []))
        ]
        if matches_for_file:
            owners[name] = sorted(matches_for_file)
        else:
            unowned.append(name)
    return {
        "files": len(owners) + len(unowned),
        "owners": owners,
        "unowned": sorted(unowned),
    }


def catalog_check(
    root: Path,
    catalog_path: Path | None = None,
    scenarios_path: Path | None = None,
) -> dict[str, Any]:
    """Validate the classified test catalog and its gate/scenario obligations.

    The catalog is the machine-readable representation of the responsibility-
    based test architecture. live-capture/recorder/release select classified tests; privilege is
    an environment dimension, not a layer. Fails on duplicate/unknown IDs,
    missing fields, invalid layer/environment, undefined commands, selector
    membership inconsistencies, superset violations, and unmapped scenario or
    legacy-check obligations.
    """
    if tomllib is None:
        raise RuntimeError("Python 3.11+ is required (tomllib missing)")
    path = catalog_path or root / "validation/test-architecture/test-catalog.toml"
    with path.open("rb") as handle:
        catalog = tomllib.load(handle)
    issues: list[str] = []
    if catalog.get("version") != 1:
        issues.append(f"unsupported catalog version {catalog.get('version')}")
    layers = set(catalog.get("layers", []))
    environments = set(catalog.get("environments", []))
    selectors = set(catalog.get("selectors", []))
    tests = catalog.get("test", [])
    ids: set[str] = set()
    declared_gates: dict[str, set[str]] = {}
    statuses = {"existing", "planned"}
    for index, test in enumerate(tests):
        label = f"test[{index}]"
        test_id = test.get("id")
        if not test_id:
            issues.append(f"{label}: missing id")
            continue
        if test_id in ids:
            issues.append(f"duplicate test id: {test_id}")
        ids.add(test_id)
        for field in ("layer", "environment", "owner", "command", "status"):
            if not test.get(field):
                issues.append(f"test {test_id}: missing {field}")
        if test.get("layer") not in layers:
            issues.append(f"test {test_id}: invalid layer {test.get('layer')!r}")
        if test.get("environment") not in environments:
            issues.append(
                f"test {test_id}: invalid environment {test.get('environment')!r}"
            )
        if test.get("status") not in statuses:
            issues.append(f"test {test_id}: invalid status {test.get('status')!r}")
        gates = test.get("gates", [])
        unknown_gates = [gate for gate in gates if gate not in selectors]
        if unknown_gates:
            issues.append(f"test {test_id}: unknown gate(s) {sorted(unknown_gates)}")
        declared_gates[test_id] = set(gates)
    required = catalog.get("required", {})
    for selector in sorted(required):
        if selector not in selectors:
            issues.append(f"unknown selector {selector!r} in required section")
            continue
        for test_id in required[selector].get("tests", []):
            if test_id not in ids:
                issues.append(
                    f"selector {selector}: required test {test_id} is undefined"
                )
            elif selector not in declared_gates.get(test_id, set()):
                issues.append(
                    f"selector {selector}: required test {test_id} does not declare gate {selector}"
                )
    live_capture = set(required.get("live-capture", {}).get("tests", []))
    recorder = set(required.get("recorder", {}).get("tests", []))
    release = set(required.get("release", {}).get("tests", []))
    for name, smaller, larger in (
        ("live-capture", live_capture, recorder),
        ("recorder", recorder, release),
    ):
        missing = sorted(smaller - larger)
        if missing:
            issues.append(
                f"selector {name} required coverage missing from superset: {missing}"
            )
    resolved_scenarios = scenarios_path or root / "scripts/acceptance/scenarios.toml"
    scenario_def: dict[str, Any] = {}
    try:
        with resolved_scenarios.open("rb") as handle:
            scenario_def = tomllib.load(handle)
    except OSError as exc:
        issues.append(f"cannot read scenarios {resolved_scenarios}: {exc}")
    scenario_ids = set(scenario_def.get("scenarios", {}))
    scenario_tests = catalog.get("scenario_tests", {})
    uncovered = sorted(scenario_ids - set(scenario_tests))
    if uncovered:
        issues.append(f"scenario(s) without classified coverage: {uncovered}")
    for scenario, mapped in sorted(scenario_tests.items()):
        if scenario not in scenario_ids:
            issues.append(f"scenario_tests references unknown scenario {scenario!r}")
        for test_id in mapped:
            if test_id not in ids:
                issues.append(
                    f"scenario {scenario}: mapped test {test_id} is undefined"
                )
    legacy_checks: set[str] = set()
    for cfg in scenario_def.get("scenarios", {}).values():
        legacy_checks.update(cfg.get("legacy_live_capture_checks", []))
        legacy_checks.update(cfg.get("legacy_recorder_checks", []))
    legacy_map = catalog.get("legacy_check_tests", {})
    unmapped = sorted(legacy_checks - set(legacy_map))
    if unmapped:
        issues.append(f"legacy check(s) without classified coverage: {unmapped}")
    for check, mapped in sorted(legacy_map.items()):
        if check not in legacy_checks:
            issues.append(f"legacy_check_tests references unknown check {check!r}")
        for test_id in mapped:
            if test_id not in ids:
                issues.append(
                    f"legacy check {check}: mapped test {test_id} is undefined"
                )
    status_by_id = {entry.get("id"): entry.get("status") for entry in tests}
    planned_required = sorted(
        {
            test_id
            for selector in required
            for test_id in required.get(selector, {}).get("tests", [])
            if status_by_id.get(test_id) == "planned"
        }
    )
    return {
        "catalog": str(path.relative_to(root)),
        "tests": len(tests),
        "selectors": sorted(selectors),
        "layers": sorted(layers),
        "environments": sorted(environments),
        "live_capture_required": len(live_capture),
        "recorder_required": len(recorder),
        "release_required": len(release),
        "scenarios": len(scenario_ids),
        "planned_required": planned_required,
        "issues": issues,
    }


def files_for_patterns(root: Path, patterns: list[str]) -> list[Path]:
    result: set[Path] = set()
    tracked = run(root, "git", "ls-files").splitlines()
    for name in tracked:
        if any(matches(name, pattern) for pattern in patterns):
            path = root / name
            if path.is_file():
                result.add(path)
    # Git-aware lookup above covers tracked files. Walk the checkout as well so
    # untracked acceptance-sensitive source is fingerprinted exactly as tested.
    ignored = {
        ".git",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
        ".venv",
        "__pycache__",
        "graphify-out",
        "node_modules",
        "target",
    }
    for directory, directories, names in os.walk(root):
        directories[:] = [name for name in directories if name not in ignored]
        base = Path(directory)
        for name in names:
            if name == ".DS_Store" or name.endswith((".pyc", ".pyo")):
                continue
            path = base / name
            relative = str(path.relative_to(root))
            if any(matches(relative, pattern) for pattern in patterns):
                result.add(path)
    return sorted(result)


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def environment(root: Path) -> dict[str, Any]:
    os_release: dict[str, str] = {}
    capabilities: dict[str, str] = {}
    status = Path("/proc/self/status")
    if status.is_file():
        for line in status.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith(("CapPrm:", "CapEff:", "CapBnd:")):
                key, value = line.split(":", 1)
                capabilities[key] = value.strip()
    release = Path("/etc/os-release")
    if release.is_file():
        for line in release.read_text(encoding="utf-8", errors="replace").splitlines():
            if "=" in line:
                key, value = line.split("=", 1)
                os_release[key] = value.strip('"')
    return {
        "rustc": run(root, "rustc", "-Vv"),
        "architecture": platform.machine(),
        "kernel": platform.release(),
        "ubuntu": os_release.get("VERSION_ID", "not_checked"),
        "os": platform.system(),
        "target": os.environ.get("CARGO_BUILD_TARGET", "host"),
        "btf": Path("/sys/kernel/btf/vmlinux").is_file(),
        "cgroup_v2": Path("/sys/fs/cgroup/cgroup.controllers").is_file(),
        "capabilities": capabilities,
    }


def load_environment(root: Path, path: Path | None = None) -> dict[str, Any]:
    if path is None:
        return environment(root)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"invalid environment JSON: {path}") from exc
    if not isinstance(value, dict):
        raise ValueError("environment JSON must contain an object")
    return value


def fingerprint(
    root: Path,
    gate: str,
    cfg: dict[str, Any],
    environment_override: dict[str, Any] | None = None,
    *,
    profile: str | None = None,
    executor: str | None = None,
) -> dict[str, Any]:
    gate_cfg = cfg.get("gates", {}).get(gate)
    if not gate_cfg:
        raise ValueError(f"unknown gate: {gate}")
    groups = cfg.get("groups", {})
    patterns: list[str] = []
    for group_name in gate_cfg.get("source_groups", []):
        patterns.extend(groups.get(group_name, {}).get("paths", []))
    patterns.extend(gate_cfg.get("acceptance_paths", []))
    patterns.extend(gate_cfg.get("build_inputs", []))
    patterns.append("Cargo.lock")
    paths = files_for_patterns(root, sorted(set(patterns)))
    inputs = [
        {"path": str(path.relative_to(root)), "sha256": digest(path)} for path in paths
    ]
    payload = {
        "fingerprint_version": 2,
        "version": cfg.get("version", 1),
        "gate": gate,
        "profile": profile or gate,
        "executor": executor or "validation",
        "inputs": inputs,
        # Environment is compatibility metadata, not source identity. Keeping it
        # outside the hashed payload permits equivalent source trees to reuse only
        # when the executor separately proves environment compatibility.
        "environment": environment_override or environment(root),
        "validation_config_sha256": digest(root / "validation/groups.toml"),
        "checks": gate_cfg.get("checks", []),
    }
    fingerprint_payload = {
        key: value for key, value in payload.items() if key != "environment"
    }
    encoded = json.dumps(
        fingerprint_payload, sort_keys=True, separators=(",", ":")
    ).encode()
    return {"fingerprint": hashlib.sha256(encoded).hexdigest(), **payload}


def acceptance_fingerprint(root: Path, cfg: dict[str, Any]) -> dict[str, Any]:
    """Fingerprint union of live-capture/recorder acceptance-sensitive source."""
    patterns: list[str] = []
    checks: set[str] = set()
    for gate in ("live-capture", "recorder"):
        gate_cfg = cfg.get("gates", {}).get(gate, {})
        for group_name in gate_cfg.get("source_groups", []):
            patterns.extend(cfg.get("groups", {}).get(group_name, {}).get("paths", []))
        patterns.extend(gate_cfg.get("acceptance_paths", []))
        patterns.extend(gate_cfg.get("build_inputs", []))
        checks.update(gate_cfg.get("checks", []))
    patterns = [pattern for pattern in patterns if not pattern.startswith("docs/")]
    patterns.extend(
        [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "crates/**",
            "ebpf/**",
            "scripts/acceptance.sh",
            "scripts/acceptance/**",
            "scripts/validation.py",
            "scripts/validate.sh",
            "validation/**",
        ]
    )
    paths = files_for_patterns(root, sorted(set(patterns)))
    inputs = [
        {"path": str(path.relative_to(root)), "sha256": digest(path)} for path in paths
    ]
    payload = {
        "fingerprint_version": 2,
        "inputs": inputs,
        "validation_config_sha256": digest(root / "validation/groups.toml"),
        "checks": sorted(checks),
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    return {"fingerprint": hashlib.sha256(encoded).hexdigest(), **payload}


def iso_now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()


def copy_failure_data(source: Path, dest: Path) -> list[str]:
    copied: list[str] = []
    report = source / "acceptance-report.json"
    if report.is_file():
        target = dest / "failure-summary.md"
        target.write_text(
            "# Validation failure\n\n```json\n"
            + report.read_text(encoding="utf-8", errors="replace")
            + "\n```\n",
            encoding="utf-8",
        )
        copied.append(str(target.relative_to(dest)))
    candidates = [
        "failed-test.log",
        "failure.log",
        "kernel-log.txt",
        "reproducer.sh",
        "commands.log",
        "build.log",
        "recorder-status.json",
        "recorder-status.stderr",
        "recorder-service-status.txt",
        "recorder-journal.log",
        "kernel-version.txt",
        "kernel-capabilities.json",
        "cgroup-information.txt",
        "btf-information.txt",
        "bpftool-programs.txt",
        "bpftool-links.txt",
        "wal-directory-listing.txt",
        "checkpoint-metadata.json",
        "process-list.txt",
        "disk-space.txt",
        "readiness-transitions.log",
    ]
    for name in candidates:
        path = source / name
        if path.is_file():
            target_name = name
            if (
                name in {"commands.log", "build.log"}
                and (dest / "failed-test.log").exists()
            ):
                continue
            if name in {"commands.log", "build.log"}:
                target_name = "failed-test.log"
            shutil.copy2(path, dest / target_name)
            copied.append(target_name)
    if not (dest / "failed-test.log").exists():
        logs = sorted(source.rglob("*.log"))
        if logs:
            shutil.copy2(logs[0], dest / "failed-test.log")
            copied.append("failed-test.log")
    paths_file = source / "failure-paths.txt"
    selected: list[Path] = []
    if paths_file.is_file():
        for line in paths_file.read_text(
            encoding="utf-8", errors="replace"
        ).splitlines():
            path = (source / line).resolve()
            try:
                path.relative_to(source.resolve())
            except ValueError:
                continue
            if path.is_file():
                selected.append(path)
    if selected:
        staging = dest / "relevant-wal-segments"
        for path in selected:
            relative = path.relative_to(source.resolve())
            target = staging / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
        archive = dest / "relevant-wal-segments.tar.zst"
        zstd = shutil.which("zstd")
        if zstd:
            with tempfile.NamedTemporaryFile(suffix=".tar", delete=False) as handle:
                tar_path = Path(handle.name)
            try:
                with tarfile.open(tar_path, "w") as tar:
                    tar.add(staging, arcname="relevant-wal-segments")
                subprocess.run(
                    [zstd, "-q", "-f", str(tar_path), "-o", str(archive)], check=True
                )
                shutil.rmtree(staging)
                tar_path.unlink(missing_ok=True)
                copied.append(archive.name)
            except (OSError, subprocess.CalledProcessError):
                tar_path.unlink(missing_ok=True)
        if staging.exists():
            copied.append(str(staging.relative_to(dest)))
    return copied


def compact(args: argparse.Namespace) -> None:
    source, dest = Path(args.source), Path(args.dest)
    if args.artifact_mode == "no-artifact":
        return
    dest.mkdir(parents=True, exist_ok=True)
    status = args.status
    copied: list[str] = []
    if status != "passed":
        copied = copy_failure_data(source, dest)
    # Successful evidence stays compact locally. Full release artifacts belong in
    # an external evidence store, never in the repository workspace.
    elif args.artifact_mode == "release":
        copied = []
    run_environment = load_environment(source, getattr(args, "environment", None))
    artifact_policy = (
        "compact-success-external-full"
        if args.artifact_mode == "release"
        else "failure-first"
    )
    summary = {
        "version": 1,
        "gate": args.gate,
        "status": status,
        "fingerprint": args.fingerprint,
        "original_commit": args.commit,
        "checks": [check for check in args.checks.split(",") if check],
        "artifact_policy": artifact_policy,
    }
    (dest / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (dest / "environment.json").write_text(
        json.dumps(run_environment, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    manifest = {
        "version": 1,
        "gate": args.gate,
        "status": status,
        "fingerprint": args.fingerprint,
        "original_commit": args.commit,
        "created_at": iso_now(),
        "environment": run_environment,
        "covered_checks": [check for check in args.checks.split(",") if check],
        "executed": status == "passed",
        "reused": False,
        "files": ["summary.json", "environment.json", "manifest.json", *copied],
        "artifact_policy": artifact_policy,
    }
    (dest / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    files = [
        path
        for path in sorted(dest.rglob("*"))
        if path.is_file() and path.name != "checksums.txt"
    ]
    (dest / "checksums.txt").write_text(
        "".join(f"{digest(path)}  {path.relative_to(dest)}\n" for path in files),
        encoding="utf-8",
    )


def reuse(args: argparse.Namespace) -> int:
    manifest_path = Path(args.evidence_root) / args.gate / "manifest.json"
    if not manifest_path.is_file():
        return 1
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        if (
            manifest.get("version") != 1
            or manifest.get("gate") != args.gate
            or manifest.get("status") != "passed"
            or not isinstance(manifest.get("executed"), bool)
            or not manifest.get("executed")
            or manifest.get("fingerprint") != args.fingerprint
            or manifest.get("environment") != environment(Path.cwd())
        ):
            return 1
        required_checks = {
            check for check in getattr(args, "checks", "").split(",") if check
        }
        covered = manifest.get("covered_checks")
        if not isinstance(covered, list) or not all(
            isinstance(check, str) for check in covered
        ):
            return 1
        covered_checks = set(covered)
        if not required_checks.issubset(covered_checks):
            return 1
        checksums = manifest_path.parent / "checksums.txt"
        if not checksums.is_file():
            return 1
        root = manifest_path.parent.resolve()
        listed: set[Path] = set()
        for line in checksums.read_text(encoding="utf-8").splitlines():
            expected, relative = line.split("  ", 1)
            candidate = Path(relative)
            if (
                candidate.is_absolute()
                or ".." in candidate.parts
                or candidate in listed
            ):
                return 1
            path = (root / candidate).resolve()
            try:
                path.relative_to(root)
            except ValueError:
                return 1
            if not path.is_file() or digest(path) != expected:
                return 1
            listed.add(candidate)
        actual = {
            path.relative_to(root)
            for path in root.rglob("*")
            if path.is_file() and path.name != "checksums.txt"
        }
        if listed != actual or set(manifest.get("files", [])) != {
            str(path) for path in actual
        }:
            return 1
    except (OSError, ValueError, json.JSONDecodeError):
        return 1
    print(
        json.dumps(
            {
                "reused": True,
                **{
                    key: manifest.get(key)
                    for key in (
                        "fingerprint",
                        "original_commit",
                        "created_at",
                        "environment",
                        "covered_checks",
                    )
                },
            }
        )
    )
    return 0


def legacy_names_check(root: Path) -> dict[str, Any]:
    """Reject production references to removed pre-release migration models.

    Before 0.1.0, unreleased internal schemas are replaced rather than carried
    through permanent runtime compatibility layers. Any production source that
    names a removed legacy type or adapter is migration baggage and fails the
    fast gate so it cannot return without an explicit OpenSpec change.
    """
    removed = [
        "EpochCatalogV1",
        "EpochCatalogV2",
        "EpochCatalogEntryV2",
        "EpochCatalogSummaryV2",
        "LegacyOneEpochMapping",
        "RolloverTransitionV1",
        "RolloverTransitionV2",
        "RolloverTransitionV2Phase",
        "load_transition_v2",
        "write_transition_v2_atomic",
        "IncrementalEtlCheckpointV1",
        "IncrementalEtlCheckpointV2",
        "INCREMENTAL_CHECKPOINT_V2_SCHEMA_VERSION",
        "ContinuationDependencyV2",
        "EpochContinuationCheckpointV1",
        "RecoveryAuthoritativeSnapshot",
        "reconcile_delta_checkpoint",
        "recover_pending_checkpoint_v2",
        "publish_delta_then_checkpoint_v2",
        "read_checkpoint",
        "write_checkpoint_atomic",
        "CheckpointError",
        "record_production",
        "record_production_with_lifetime",
        "record_live_ebpf",
        "record_live_ebpf_with_lifetime",
        "recover_rollover_transition_for_root",
        "load_compatible",
        "from_legacy",
        "to_legacy",
        # Removed pre-0.1 CLI compatibility surface (crates/chronicle-cli).
        "LegacyInvocation",
        "DeprecationJson",
        "record_fixture_legacy",
        "run_replay_legacy",
        "raw_legacy_record",
        "legacy_invocation_from_raw_args",
        "legacy_error_message",
        "exit_legacy_error",
        "format_from_raw_args",
        "write_deprecation_warning",
    ]
    removed_paths = [
        # Retired standalone eBPF feasibility harness (Gate A).
        "ebpf-feasibility",
        # Retired historical feasibility documentation area.
        "docs/feasibility",
        # Retired harness loader test.
        "crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs",
    ]
    issues: list[str] = []
    for path in sorted((root / "crates").rglob("*.rs")):
        if any(part in {"target", "testdata", "fixtures"} for part in path.parts):
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for name in removed:
            # Word-boundary match so intentional adapters such as the WAL v1
            # identity bridge (from_legacy_uuid) are not flagged.
            if re.search(rf"\b{re.escape(name)}\b", text):
                issues.append(
                    f"{path.relative_to(root)}: references removed legacy {name}"
                )
    for relative in removed_paths:
        if (root / relative).exists():
            issues.append(
                f"{relative}: retired pre-release path must not return "
                "(historical stages live in Git history and archived OpenSpec changes)"
            )
    scenarios = root / "scripts/acceptance/scenarios.toml"
    if scenarios.is_file():
        text = scenarios.read_text(encoding="utf-8", errors="replace")
        if "cli-compatibility" in text:
            issues.append(
                "scripts/acceptance/scenarios.toml: removed cli-compatibility "
                "scenario must not return"
            )
    for spec in sorted((root / "openspec/specs").glob("*/spec.md")):
        text = spec.read_text(encoding="utf-8", errors="replace")
        # The exact mandate that kept the removed pre-release surface alive in
        # specs (runnable-http-cli). Descriptive uses of "legacy" (for
        # example "removed before 0.1.0") are not flagged.
        if "SHALL retain the legacy" in text:
            issues.append(
                f"{spec.relative_to(root)}: spec mandates a removed pre-release "
                "CLI compatibility surface"
            )
    return {"issues": issues}


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    select_parser = sub.add_parser("select")
    select_parser.add_argument("--root", type=Path, required=True)
    select_parser.add_argument("--changed-since")
    select_parser.add_argument("--paths", nargs="*")
    select_parser.add_argument("--config", type=Path)
    environment_parser = sub.add_parser("environment")
    environment_parser.add_argument("--root", type=Path, required=True)
    ownership_parser = sub.add_parser("ownership")
    legacy_parser = sub.add_parser("legacy-names")
    legacy_parser.add_argument("--root", type=Path, required=True)
    architecture_parser = sub.add_parser("architecture")
    architecture_parser.add_argument("--root", type=Path, required=True)
    architecture_parser.add_argument("--config", type=Path)
    ownership_parser.add_argument("--root", type=Path, required=True)
    ownership_parser.add_argument("--config", type=Path)
    fp_parser = sub.add_parser("fingerprint")
    fp_parser.add_argument("--root", type=Path, required=True)
    fp_parser.add_argument("--gate", required=True)
    fp_parser.add_argument("--config", type=Path)
    fp_parser.add_argument("--environment", type=Path)
    compact_parser = sub.add_parser("compact")
    compact_parser.add_argument("--source", required=True)
    compact_parser.add_argument("--dest", required=True)
    compact_parser.add_argument("--gate", required=True)
    compact_parser.add_argument("--status", required=True)
    compact_parser.add_argument("--fingerprint", required=True)
    compact_parser.add_argument("--commit", required=True)
    compact_parser.add_argument("--checks", default="")
    compact_parser.add_argument("--environment", type=Path)
    compact_parser.add_argument(
        "--artifact-mode",
        choices=("no-artifact", "artifact-on-failure", "release"),
        required=True,
    )
    reuse_parser = sub.add_parser("reuse")
    reuse_parser.add_argument("--evidence-root", required=True)
    reuse_parser.add_argument("--gate", required=True)
    reuse_parser.add_argument("--fingerprint", required=True)
    reuse_parser.add_argument("--checks", default="")
    reuse_parser.add_argument("--commit")
    reuse_parser.add_argument("--release", action="store_true")
    catalog_parser = sub.add_parser("catalog")
    catalog_parser.add_argument("--root", type=Path, required=True)
    catalog_parser.add_argument("--catalog", type=Path)
    catalog_parser.add_argument("--scenarios", type=Path)
    args = parser.parse_args()
    if args.command == "environment":
        print(json.dumps(environment(args.root), indent=2, sort_keys=True))
    elif args.command == "legacy-names":
        value = legacy_names_check(args.root)
        print(json.dumps(value, indent=2, sort_keys=True))
        return 1 if value["issues"] else 0
    elif args.command == "architecture":
        value = architecture_check(args.root, args.config)
        print(json.dumps(value, indent=2, sort_keys=True))
        return 1 if value["issues"] else 0
    elif args.command == "ownership":
        value = source_ownership(args.root, config(args.root, args.config))
        print(json.dumps(value, indent=2, sort_keys=True))
        if value["unowned"]:
            return 1
    elif args.command == "select":
        cfg = config(args.root, args.config)
        paths = (
            args.paths
            if args.paths is not None and args.paths
            else changed_paths(args.root, args.changed_since)
        )
        print(
            json.dumps(
                select(args.root, sorted(set(paths)), cfg), indent=2, sort_keys=True
            )
        )
    elif args.command == "catalog":
        value = catalog_check(args.root, args.catalog, args.scenarios)
        print(json.dumps(value, indent=2, sort_keys=True))
        return 1 if value["issues"] else 0
    elif args.command == "fingerprint":
        print(
            json.dumps(
                fingerprint(
                    args.root,
                    args.gate,
                    config(args.root, args.config),
                    load_environment(args.root, args.environment),
                ),
                indent=2,
                sort_keys=True,
            )
        )
    elif args.command == "compact":
        compact(args)
    else:
        return reuse(args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
