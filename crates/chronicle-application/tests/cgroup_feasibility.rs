#![cfg(target_os = "linux")]

use std::collections::BTreeSet;

#[derive(Clone, Copy)]
struct ProcessEvidence {
    tgid: Option<u32>,
    pgid: u32,
    namespace_levels: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rejection {
    ForbiddenRoot,
    Unreadable,
    EnumerationRace,
    NamespaceAmbiguous,
    RecorderInsideSelection,
    SharedWithoutAcknowledgement,
    SelectedPidTgidMismatch,
}

#[derive(Debug, PartialEq, Eq)]
struct ScopeEvidence {
    direct_tgids: BTreeSet<u32>,
    descendant_cgroups: usize,
}

#[allow(clippy::fn_params_excessive_bools)]
fn evaluate_scope(
    processes: &[ProcessEvidence],
    descendant_cgroups: usize,
    readable: bool,
    forbidden_root: bool,
    recorder_inside: bool,
    allow_shared: bool,
) -> Result<ScopeEvidence, Rejection> {
    if forbidden_root {
        return Err(Rejection::ForbiddenRoot);
    }
    if !readable {
        return Err(Rejection::Unreadable);
    }
    if recorder_inside {
        return Err(Rejection::RecorderInsideSelection);
    }
    if processes.iter().any(|process| process.tgid.is_none()) {
        return Err(Rejection::EnumerationRace);
    }
    if processes
        .iter()
        .any(|process| process.namespace_levels != 1)
    {
        return Err(Rejection::NamespaceAmbiguous);
    }
    let direct_tgids: BTreeSet<_> = processes
        .iter()
        .filter_map(|process| process.tgid)
        .collect();
    if (direct_tgids.len() > 1 || descendant_cgroups > 0) && !allow_shared {
        return Err(Rejection::SharedWithoutAcknowledgement);
    }
    Ok(ScopeEvidence {
        direct_tgids,
        descendant_cgroups,
    })
}

fn evaluate_pid_scope(
    selected_tgid: u32,
    processes: &[ProcessEvidence],
    descendant_cgroups: usize,
) -> Result<ScopeEvidence, Rejection> {
    let evidence = evaluate_scope(processes, descendant_cgroups, true, false, false, true)?;
    if evidence.direct_tgids != BTreeSet::from([selected_tgid]) {
        return Err(Rejection::SelectedPidTgidMismatch);
    }
    Ok(evidence)
}

fn explicit_scope_warning(
    processes: &[ProcessEvidence],
    descendant_cgroups: usize,
    acknowledged: bool,
) -> Result<String, Rejection> {
    let evidence = evaluate_scope(
        processes,
        descendant_cgroups,
        true,
        false,
        false,
        acknowledged,
    )?;
    Ok(format!(
        "direct TGID count={} descendant cgroup count={} selected subtree",
        evidence.direct_tgids.len(),
        evidence.descendant_cgroups
    ))
}

fn process(tgid: u32, pgid: u32) -> ProcessEvidence {
    ProcessEvidence {
        tgid: Some(tgid),
        pgid,
        namespace_levels: 1,
    }
}

#[test]
fn multithreaded_process_deduplicates_host_visible_tgid() {
    let evidence = evaluate_scope(
        &[process(41, 41), process(41, 41), process(41, 41)],
        0,
        true,
        false,
        false,
        false,
    )
    .unwrap();
    assert_eq!(evidence.direct_tgids, BTreeSet::from([41]));
}

#[test]
fn unrelated_tgids_and_descendants_require_explicit_acknowledgement() {
    assert_eq!(
        evaluate_scope(
            &[process(41, 41), process(42, 42)],
            0,
            true,
            false,
            false,
            false,
        ),
        Err(Rejection::SharedWithoutAcknowledgement)
    );
    assert_eq!(
        evaluate_scope(&[], 1, true, false, false, false),
        Err(Rejection::SharedWithoutAcknowledgement)
    );
    assert_eq!(
        evaluate_scope(
            &[process(41, 41), process(42, 42)],
            1,
            true,
            false,
            false,
            true,
        )
        .unwrap(),
        ScopeEvidence {
            direct_tgids: BTreeSet::from([41, 42]),
            descendant_cgroups: 1,
        }
    );
}

#[test]
fn shared_posix_pgid_does_not_merge_distinct_tgids() {
    let processes = [process(41, 900), process(42, 900)];
    assert_eq!(processes[0].pgid, processes[1].pgid);
    assert_eq!(
        evaluate_scope(&processes, 0, true, false, false, false),
        Err(Rejection::SharedWithoutAcknowledgement)
    );
}

#[test]
fn races_namespaces_recorder_and_unreadable_scope_fail_closed() {
    let exited = ProcessEvidence {
        tgid: None,
        pgid: 1,
        namespace_levels: 1,
    };
    let namespaced = ProcessEvidence {
        tgid: Some(41),
        pgid: 41,
        namespace_levels: 2,
    };
    assert_eq!(
        evaluate_scope(&[exited], 0, true, false, false, true),
        Err(Rejection::EnumerationRace)
    );
    assert_eq!(
        evaluate_scope(&[namespaced], 0, true, false, false, true),
        Err(Rejection::NamespaceAmbiguous)
    );
    assert_eq!(
        evaluate_scope(&[], 0, true, false, true, true),
        Err(Rejection::RecorderInsideSelection)
    );
    assert_eq!(
        evaluate_scope(&[], 0, false, false, false, true),
        Err(Rejection::Unreadable)
    );
}

#[test]
fn pid_selector_rejects_unrelated_host_visible_tgid_even_with_shared_ack() {
    assert_eq!(
        evaluate_pid_scope(41, &[process(41, 900), process(42, 900)], 0),
        Err(Rejection::SelectedPidTgidMismatch)
    );
    assert_eq!(
        evaluate_pid_scope(41, &[process(41, 900)], 0)
            .unwrap()
            .direct_tgids,
        BTreeSet::from([41])
    );
}

#[test]
fn explicit_scope_warning_is_count_only_and_requires_acknowledgement() {
    let processes = [process(41, 900), process(42, 900)];
    assert_eq!(
        explicit_scope_warning(&processes, 1, false),
        Err(Rejection::SharedWithoutAcknowledgement)
    );
    let warning = explicit_scope_warning(&processes, 1, true).unwrap();
    assert_eq!(
        warning,
        "direct TGID count=2 descendant cgroup count=1 selected subtree"
    );
    assert!(!warning.contains("command"));
    assert!(!warning.contains("environment"));
}

#[test]
fn acknowledgement_never_overrides_forbidden_root() {
    assert_eq!(
        evaluate_scope(
            &[process(41, 41), process(42, 42)],
            1,
            true,
            true,
            false,
            true,
        ),
        Err(Rejection::ForbiddenRoot)
    );
}
