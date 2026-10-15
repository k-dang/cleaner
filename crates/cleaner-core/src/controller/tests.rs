use super::*;
use crate::results::{CleanStatus, Problem};

type Controller = super::Controller<Vec<u64>>;
type Command = super::Command<Vec<u64>>;
type Event = crate::results::Event<Vec<u64>>;

static TARGETS: [Target; 4] = [
    Target {
        id: "cache",
        name: "Cache",
        category: "System",
        default_selected: true,
        min_age: None,
    },
    Target {
        id: "temp",
        name: "Temp",
        category: "System",
        default_selected: true,
        min_age: None,
    },
    Target {
        id: "discarded",
        name: "Discarded",
        category: "System",
        default_selected: true,
        min_age: None,
    },
    Target {
        id: "build",
        name: "Build cache",
        category: "Developer",
        default_selected: false,
        min_age: None,
    },
];

fn scanned(id: TargetId, result: ScanResult) -> Event {
    Event::Scanned(id, result, vec![])
}

/// A controller whose stored Selection is already loaded and saved.
fn loaded(pick: impl Fn(&Target) -> bool) -> Controller {
    let mut controller = Controller::new(&TARGETS);
    let save = controller.load_selection(Ok(Loaded {
        choices: selection::choices(&TARGETS, pick),
        needs_save: false,
    }));
    assert_eq!(save, None);
    controller
}

/// Picks only two fixture Targets, so tests can finish a Scan with two reports.
fn temps(target: &Target) -> bool {
    matches!(target.id, "cache" | "temp")
}

fn cleaned(deleted_bytes: u64) -> CleanResult {
    CleanResult {
        status: CleanStatus::Complete,
        deleted_bytes: Some(deleted_bytes),
        skipped: vec![],
        coverage_problem: None,
    }
}

fn scan_op(controller: &mut Controller) -> OpId {
    match controller.start_scan().unwrap() {
        Command::Scan { op, .. } => op,
        _ => unreachable!(),
    }
}

#[test]
fn selected_targets_scan_first_and_incomplete_results_block_clean() {
    let mut controller = loaded(|target| target.id == "temp");
    let Command::Scan { op, targets } = controller.start_scan().unwrap() else {
        unreachable!()
    };
    assert_eq!(targets[..2], ["temp", "cache"]);
    assert_eq!(targets.len(), TARGETS.len());
    controller.apply(
        op,
        scanned(
            "temp",
            ScanResult::Partial {
                bytes: 5,
                problem: Problem::AccessDenied,
            },
        ),
    );
    assert!(!controller.can_clean());
    controller.apply(op, Event::Finished);
    assert!(!controller.can_clean());
    assert!(controller.toggle("temp").is_some());
    assert!(controller.toggle("cache").is_some());
    controller.save_finished(Ok(()));
    controller.save_finished(Ok(()));
    assert!(
        !controller.can_clean(),
        "an unscanned selected Target is not ready"
    );
}

#[test]
fn finished_scan_checks_only_selected_targets_and_requires_one_complete() {
    let failed = ScanResult::Failed {
        problem: Problem::AccessDenied,
    };
    for (selected, result, ready) in [
        (true, ScanResult::Complete { bytes: 0 }, true),
        (true, ScanResult::NotPresent, false),
        (false, ScanResult::Complete { bytes: 10 }, false),
        (true, failed.clone(), false),
        (
            true,
            ScanResult::Partial {
                bytes: 5,
                problem: Problem::AccessDenied,
            },
            false,
        ),
        (true, ScanResult::Stopped, false),
    ] {
        let mut controller = loaded(|target| selected && target.id == "cache");
        let scan = scan_op(&mut controller);
        controller.apply(scan, scanned("cache", result.clone()));
        controller.apply(scan, scanned("temp", failed.clone()));
        controller.apply(scan, Event::Finished);
        assert_eq!(controller.can_clean(), ready, "{selected}: {result:?}");
        assert_eq!(controller.request_clean().is_some(), ready);
    }
}

#[test]
fn clean_requires_a_fresh_request_after_scan_finishes_then_rescans() {
    let mut controller = loaded(|target| target.id == "cache");
    let scan = scan_op(&mut controller);
    controller.apply(scan, scanned("cache", ScanResult::Complete { bytes: 10 }));
    assert!(!controller.can_clean());
    assert_eq!(controller.request_clean(), None);
    assert!(!controller.selection_locked());
    assert!(controller.toggle("temp").is_some());
    controller.save_finished(Ok(()));
    for id in ["temp", "discarded", "build"] {
        controller.apply(scan, scanned(id, ScanResult::NotPresent));
    }
    assert!(!controller.can_clean(), "all reports still need Finished");
    assert_eq!(controller.request_clean(), None);
    assert_eq!(controller.apply(scan, Event::Finished), None);
    assert!(controller.can_clean());
    let Some(Command::Clean { op, targets }) = controller.request_clean() else {
        panic!("expected Clean")
    };
    assert!(controller.toggle("temp").is_none());
    assert_eq!(controller.request_clean(), None);
    assert_eq!(
        targets,
        [CleanTarget {
            id: "cache",
            snapshot: vec![]
        }]
    );
    assert_eq!(
        (
            controller.summary().cleaned_targets,
            controller.summary().clean_targets
        ),
        (0, 1)
    );
    controller.apply(op, Event::Cleaned("cache", cleaned(10)));
    assert_eq!(
        (
            controller.summary().cleaned_targets,
            controller.summary().clean_targets
        ),
        (1, 1)
    );
    assert!(matches!(
        controller.apply(op, Event::Finished),
        Some(Command::Scan { .. })
    ));
    assert_eq!(controller.last_clean(), Some(&[cleaned(10)][..]));
}

#[test]
fn clean_receives_only_selected_snapshots_from_the_latest_scan() {
    let mut controller = loaded(|target| target.id == "discarded");
    let first = scan_op(&mut controller);
    controller.apply(
        first,
        Event::Scanned("discarded", ScanResult::Complete { bytes: 0 }, vec![1, 2]),
    );
    controller.apply(first, Event::Finished);
    let Some(Command::Clean { op, targets }) = controller.request_clean() else {
        panic!("expected Clean")
    };
    assert_eq!(
        targets,
        [CleanTarget {
            id: "discarded",
            snapshot: vec![1, 2]
        }]
    );
    controller.apply(op, Event::Cleaned("discarded", cleaned(0)));
    let Some(Command::Scan { op: latest, .. }) = controller.apply(op, Event::Finished) else {
        panic!("expected rescan")
    };
    assert!(
        !controller.can_clean(),
        "old snapshots cannot make a new Scan ready"
    );
    controller.apply(
        latest,
        Event::Scanned("discarded", ScanResult::Complete { bytes: 0 }, vec![3]),
    );
    controller.apply(
        latest,
        Event::Scanned("cache", ScanResult::Complete { bytes: 9 }, vec![4]),
    );
    controller.apply(
        first,
        Event::Scanned("discarded", ScanResult::Complete { bytes: 0 }, vec![99]),
    );
    controller.apply(latest, Event::Finished);
    let Some(Command::Clean { targets, .. }) = controller.request_clean() else {
        panic!("expected Clean")
    };
    assert_eq!(
        targets,
        [CleanTarget {
            id: "discarded",
            snapshot: vec![3]
        }]
    );
}

#[test]
fn manual_and_post_clean_rescans_estimate_only_current_reports() {
    for clean_first in [false, true] {
        let mut controller = loaded(temps);
        let scan = scan_op(&mut controller);
        controller.apply(scan, scanned("cache", ScanResult::Complete { bytes: 4 }));
        controller.apply(scan, scanned("temp", ScanResult::Complete { bytes: 9 }));
        controller.apply(scan, Event::Finished);
        assert_eq!(controller.summary().selected_bytes, Some(13));

        let rescan = if clean_first {
            let Some(Command::Clean { op, .. }) = controller.request_clean() else {
                panic!("expected Clean")
            };
            controller.apply(op, Event::Cleaned("cache", cleaned(4)));
            controller.apply(op, Event::Cleaned("temp", cleaned(9)));
            let Some(Command::Scan { op, .. }) = controller.apply(op, Event::Finished) else {
                panic!("expected rescan")
            };
            op
        } else {
            scan_op(&mut controller)
        };
        assert!(controller.rows().iter().all(|row| row.scan.is_none()));
        assert_eq!(controller.summary().complete_bytes, 0);
        assert_eq!(controller.summary().selected_bytes, None);

        controller.apply(rescan, Event::Scanning("cache"));
        assert_eq!(controller.scanning_target(), Some("cache"));
        controller.apply(rescan, scanned("cache", ScanResult::Complete { bytes: 1 }));
        assert_eq!(controller.scanning_target(), None);
        assert_eq!(controller.summary().complete_bytes, 1);
        assert_eq!(controller.summary().selected_bytes, Some(1));
        assert!(
            controller
                .rows()
                .iter()
                .find(|row| row.target.id == "temp")
                .unwrap()
                .scan
                .is_none()
        );
        assert!(!controller.can_clean(), "the Scan is still running");
        assert_eq!(controller.request_clean(), None);

        controller.apply(rescan, scanned("temp", ScanResult::Complete { bytes: 2 }));
        assert_eq!(controller.summary().complete_bytes, 3);
        assert_eq!(controller.summary().selected_bytes, Some(3));
        assert_eq!(controller.apply(rescan, Event::Finished), None);
        assert!(controller.can_clean());
    }
}

#[test]
fn clean_uses_visible_selection_while_saving_and_close_suppresses_rescan() {
    let mut controller = loaded(|target| target.id == "cache");
    let scan = scan_op(&mut controller);
    controller.apply(scan, scanned("cache", ScanResult::Complete { bytes: 2 }));
    controller.apply(scan, scanned("temp", ScanResult::Complete { bytes: 3 }));
    controller.apply(scan, Event::Finished);
    assert!(controller.toggle("temp").is_some());
    assert!(controller.can_clean());
    let Some(Command::Clean { op, targets }) = controller.request_clean() else {
        panic!("expected Clean")
    };
    assert_eq!(
        targets.iter().map(|target| target.id).collect::<Vec<_>>(),
        ["cache", "temp"]
    );
    assert_eq!(controller.close(), Some(Command::Stop(op)));
    assert!(controller.toggle("cache").is_none());
    assert_eq!(controller.apply(op, Event::Finished), None);
    assert!(!controller.ready_to_exit());
    controller.save_finished(Ok(()));
    assert!(controller.ready_to_exit());
}

#[test]
fn late_results_cannot_overwrite_new_scan() {
    let mut controller = loaded(|_| false);
    let first = scan_op(&mut controller);
    controller.apply(first, Event::Finished);
    let second = scan_op(&mut controller);
    controller.apply(first, scanned("cache", ScanResult::Complete { bytes: 99 }));
    assert!(controller.rows()[0].scan.is_none());
    assert_ne!(first, second);
}

#[test]
fn duplicate_requests_do_not_start_another_operation() {
    let mut controller = loaded(temps);
    let scan = scan_op(&mut controller);
    assert!(controller.start_scan().is_none());
    controller.apply(scan, scanned("cache", ScanResult::Complete { bytes: 2 }));
    controller.apply(scan, scanned("temp", ScanResult::NotPresent));
    assert_eq!(controller.request_clean(), None);
    assert_eq!(controller.apply(scan, Event::Finished), None);
    let Some(Command::Clean { op, targets }) = controller.request_clean() else {
        panic!("expected Clean")
    };
    assert_eq!(
        targets,
        [CleanTarget {
            id: "cache",
            snapshot: vec![]
        }]
    );
    assert!(controller.start_scan().is_none());
    assert_eq!(controller.request_clean(), None);
    controller.apply(op, Event::Finished);
}

#[test]
fn close_during_scan_waits_for_worker_to_stop() {
    let mut controller = loaded(temps);
    let scan = scan_op(&mut controller);
    controller.apply(scan, scanned("cache", ScanResult::Complete { bytes: 2 }));
    controller.apply(scan, scanned("temp", ScanResult::NotPresent));
    assert_eq!(controller.close(), Some(Command::Stop(scan)));
    assert!(!controller.ready_to_exit());
    assert!(controller.toggle("cache").is_none());
    assert_eq!(controller.apply(scan, Event::Finished), None);
    assert!(controller.ready_to_exit());
    assert_eq!(controller.request_clean(), None);
    assert_eq!(controller.start_scan(), None);
}

#[test]
fn finished_clean_results_follow_worker_order_and_survive_rescan() {
    let mut controller = loaded(temps);
    let scan = scan_op(&mut controller);
    for id in ["cache", "temp"] {
        controller.apply(scan, scanned(id, ScanResult::Complete { bytes: 1 }));
    }
    controller.apply(scan, Event::Finished);
    let Some(Command::Clean { op, .. }) = controller.request_clean() else {
        panic!("expected Clean")
    };
    controller.apply(op, Event::Cleaned("temp", cleaned(2)));
    controller.apply(op, Event::Cleaned("cache", cleaned(1)));
    let Some(Command::Scan { op: rescan, .. }) = controller.apply(op, Event::Finished) else {
        panic!("expected rescan")
    };
    assert_eq!(controller.last_clean(), Some(&[cleaned(2), cleaned(1)][..]));
    controller.apply(rescan, scanned("cache", ScanResult::NotPresent));
    assert_eq!(controller.last_clean().unwrap().len(), 2);
}

#[test]
fn selection_is_locked_until_loaded() {
    let mut controller = Controller::new(&TARGETS);
    assert_eq!(controller.start_scan(), None);
    assert!(controller.toggle("cache").is_none());
    controller.load_selection(Err("unreadable".into()));
    assert!(controller.rows().iter().all(|row| !row.selected));
    assert!(controller.start_scan().is_some());
}

#[test]
fn failed_save_keeps_visible_selection_and_a_later_success_clears_the_error() {
    let mut controller = loaded(temps);
    assert!(controller.toggle("cache").is_some());
    controller.save_finished(Err("disk full".into()));
    assert!(!controller.rows()[0].selected);
    assert_eq!(
        controller.selection_error(),
        Some("Selection could not be saved: disk full")
    );
    controller.toggle("build").unwrap();
    controller.save_finished(Ok(()));
    assert!(!controller.rows()[0].selected);
    assert!(controller.rows()[3].selected);
    assert!(controller.selection_error().is_none());
}

#[test]
fn rapid_edits_queue_current_choices_and_close_waits_for_every_write() {
    let mut controller = loaded(temps);
    let mut writes = Vec::new();
    for id in ["cache", "temp", "build", "cache"] {
        let choices = controller.toggle(id).unwrap();
        assert_eq!(choices, controller.choices());
        writes.push(choices);
    }
    let latest = writes.last().unwrap();
    assert!(latest["cache"]);
    assert!(!latest["temp"]);
    assert!(latest["build"]);
    controller.close();
    for result in [
        Ok(()),
        Err("disk full".into()),
        Ok(()),
        Err("disk full".into()),
    ] {
        assert!(!controller.ready_to_exit());
        controller.save_finished(result);
        assert_eq!(&controller.choices(), latest);
    }
    assert!(controller.ready_to_exit());
}

#[test]
fn close_during_selection_load_waits_for_any_initial_save() {
    let mut controller = Controller::new(&TARGETS);
    controller.close();
    assert!(!controller.ready_to_exit());
    assert!(
        controller
            .load_selection(Ok(Loaded {
                choices: selection::defaults(&TARGETS),
                needs_save: true,
            }))
            .is_some()
    );
    assert!(!controller.ready_to_exit());
    controller.save_finished(Ok(()));
    assert!(controller.ready_to_exit());

    let mut unreadable = Controller::new(&TARGETS);
    unreadable.close();
    assert!(!unreadable.ready_to_exit());
    assert_eq!(unreadable.load_selection(Err("unreadable".into())), None);
    assert!(unreadable.ready_to_exit());
}

#[test]
fn initial_save_failure_does_not_block_clean() {
    let mut controller = Controller::new(&TARGETS);
    let save = controller.load_selection(Ok(Loaded {
        choices: selection::defaults(&TARGETS),
        needs_save: true,
    }));
    assert_eq!(save, Some(selection::defaults(&TARGETS)));
    let scan = scan_op(&mut controller);
    controller.apply(scan, scanned("cache", ScanResult::Complete { bytes: 2 }));
    controller.apply(scan, scanned("temp", ScanResult::NotPresent));
    controller.apply(scan, scanned("discarded", ScanResult::NotPresent));
    controller.apply(scan, Event::Finished);
    assert!(controller.can_clean(), "pending saves do not block Clean");
    controller.save_finished(Err("access denied".into()));
    assert!(
        controller.rows()[0].selected,
        "the loaded Selection stays shown"
    );
    assert!(controller.can_clean());
    assert_eq!(
        controller.selection_error(),
        Some("Selection could not be saved: access denied")
    );
    assert!(controller.request_clean().is_some());
}

#[test]
fn selected_estimate_tracks_current_scan_progress() {
    let mut controller = loaded(temps);
    let scan = scan_op(&mut controller);
    let summary = controller.summary();
    assert_eq!(
        (summary.selected_bytes, summary.selected_pending),
        (None, true)
    );
    controller.apply(scan, scanned("cache", ScanResult::Complete { bytes: 0 }));
    let summary = controller.summary();
    assert_eq!(
        (summary.selected_bytes, summary.selected_pending),
        (Some(0), true)
    );
    controller.apply(scan, scanned("temp", ScanResult::Complete { bytes: 5 }));
    let summary = controller.summary();
    assert_eq!(
        (summary.selected_bytes, summary.selected_pending),
        (Some(5), false)
    );
    assert_eq!(
        summary.found_targets, 1,
        "complete zero estimates are known but not found content"
    );
    controller.apply(scan, Event::Finished);
    let rescan = scan_op(&mut controller);
    controller.apply(
        rescan,
        scanned(
            "temp",
            ScanResult::Partial {
                bytes: 10,
                problem: Problem::AccessDenied,
            },
        ),
    );
    let summary = controller.summary();
    assert_eq!(
        (summary.selected_bytes, summary.selected_pending),
        (None, true)
    );
    controller.toggle("cache").unwrap();
    let summary = controller.summary();
    assert_eq!(
        (summary.selected_bytes, summary.selected_pending),
        (Some(0), false)
    );
    assert!(
        !controller.can_clean(),
        "an estimate never makes an incomplete selection ready"
    );
}

#[test]
fn summary_counts_visible_selection_and_complete_current_results() {
    let mut controller = loaded(temps);
    let initial = controller.summary();
    assert_eq!(initial.scanned_targets, 0);
    assert_eq!(initial.total_targets, TARGETS.len());
    assert_eq!(initial.selected_targets, 2);
    let scan = scan_op(&mut controller);
    controller.apply(scan, scanned("cache", ScanResult::Complete { bytes: 5 }));
    controller.apply(scan, scanned("temp", ScanResult::NotPresent));
    controller.apply(scan, scanned("build", ScanResult::Complete { bytes: 30 }));
    controller.apply(
        scan,
        scanned(
            "discarded",
            ScanResult::Partial {
                bytes: 10,
                problem: Problem::AccessDenied,
            },
        ),
    );
    let summary = controller.summary();
    assert!(summary.scanning);
    assert_eq!(summary.scanned_targets, 4);
    assert_eq!(
        summary.selected_targets, 1,
        "absent choices stay saved but leave the checklist count"
    );
    assert!(!controller.rows()[1].is_visible());
    assert_eq!(summary.selected_bytes, Some(5));
    assert!(!summary.selected_pending);
    assert_eq!(summary.complete_bytes, 35, "partial estimates are excluded");
    assert_eq!(summary.found_targets, 2);
    assert_eq!(summary.incomplete, 1);
    controller.apply(scan, Event::Finished);
    assert!(!controller.summary().scanning);
    assert!(
        controller.can_clean(),
        "unselected incomplete results do not block Clean"
    );
    controller.toggle("cache").unwrap();
    let summary = controller.summary();
    assert_eq!(summary.selected_targets, 0);
    assert_eq!(summary.selected_bytes, Some(0));
    assert!(!summary.selected_pending);
}
