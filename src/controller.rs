//! Owns the Selection, Scan results, the current operation, and the last Clean
//! result. It enforces operation exclusion and Clean readiness independently of
//! the UI. Workers receive `Command`s and report back through `apply`.

use crate::results::{CleanResult, ScanResult};
use crate::selection::Choices;
use crate::targets::{TARGETS, Target, TargetId};

/// Identifies one Scan or Clean, so late reports from an earlier one are ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpId(u64);

/// Work the controller asks a worker to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Scan {
        op: OpId,
        targets: Vec<TargetId>,
    },
    Clean {
        op: OpId,
        targets: Vec<TargetId>,
    },
    /// Ask the worker running `op` to stop between filesystem steps.
    Stop(OpId),
}

/// The operation in progress. At most one runs at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Idle,
    Scanning(OpId),
    Cleaning(OpId),
}

/// One Target's Selection and latest results.
pub struct Row {
    pub target: &'static Target,
    pub selected: bool,
    /// This Target's result from the current or latest Scan; `None` until it reports.
    pub scan: Option<ScanResult>,
    pub clean: Option<CleanProgress>,
}

/// Where a Target is in the running or latest Clean.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanProgress {
    Queued,
    Cleaning,
    Done(CleanResult),
}

/// A worker's report about the operation it is running.
#[derive(Debug)]
pub enum Event {
    Scanned(TargetId, ScanResult),
    Cleaning(TargetId),
    Cleaned(TargetId, CleanResult),
    /// The worker has stopped touching the filesystem for this operation.
    Finished,
}

/// Estimates for the top summary and the Clean button.
#[derive(Debug, PartialEq, Eq)]
pub struct Totals {
    /// Sum of complete estimates; partial and unknown sizes are excluded.
    pub complete_bytes: u64,
    /// Sum of complete estimates for selected Targets.
    pub selected_bytes: u64,
    /// Targets whose latest Scan is partial, failed, or stopped.
    pub incomplete: usize,
    pub scanning: bool,
}

pub struct Controller {
    rows: Vec<Row>,
    operation: Operation,
    next_op: u64,
    /// Targets captured by a Clean request that waits for the Scan worker to stop.
    pending_clean: Option<Vec<TargetId>>,
    /// Results reported so far by the running Clean.
    clean_results: Vec<(&'static Target, CleanResult)>,
    last_clean: Option<Vec<(&'static Target, CleanResult)>>,
    selection_saved: bool,
    clean_validated: bool,
    closing: bool,
}

impl Controller {
    pub fn new(is_selected: impl Fn(&Target) -> bool, clean_validated: bool) -> Self {
        let rows = TARGETS
            .iter()
            .map(|target| Row {
                target,
                selected: is_selected(target),
                scan: None,
                clean: None,
            })
            .collect();
        Self {
            rows,
            operation: Operation::Idle,
            next_op: 0,
            pending_clean: None,
            clean_results: Vec::new(),
            last_clean: None,
            selection_saved: true,
            clean_validated,
            closing: false,
        }
    }

    /// Starts a Scan of every Target, selected ones first. Only allowed when idle.
    pub fn start_scan(&mut self) -> Option<Command> {
        if self.operation != Operation::Idle || self.closing {
            return None;
        }
        let op = self.begin_op();
        self.operation = Operation::Scanning(op);
        for row in &mut self.rows {
            row.scan = None;
        }
        let (selected, rest): (Vec<&Row>, Vec<&Row>) =
            self.rows.iter().partition(|row| row.selected);
        let targets = selected
            .iter()
            .chain(&rest)
            .map(|row| row.target.id)
            .collect();
        Some(Command::Scan { op, targets })
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn operation(&self) -> Operation {
        self.operation
    }

    pub fn totals(&self) -> Totals {
        let mut totals = Totals {
            complete_bytes: 0,
            selected_bytes: 0,
            incomplete: 0,
            scanning: matches!(self.operation, Operation::Scanning(_)),
        };
        for row in &self.rows {
            match row.scan {
                Some(ScanResult::Complete { bytes }) => {
                    totals.complete_bytes += bytes;
                    if row.selected {
                        totals.selected_bytes += bytes;
                    }
                }
                Some(
                    ScanResult::Partial { .. } | ScanResult::Failed { .. } | ScanResult::Stopped,
                ) => totals.incomplete += 1,
                Some(ScanResult::NotPresent) | None => {}
            }
        }
        totals
    }

    /// Applies a worker report. Reports from any operation but the current one are ignored.
    pub fn apply(&mut self, op: OpId, event: Event) -> Option<Command> {
        match (self.operation, event) {
            (Operation::Scanning(current), Event::Scanned(id, result)) if current == op => {
                let row = self.row_mut(id);
                row.scan = Some(result);
                row.clean = None;
                None
            }
            (Operation::Scanning(current), Event::Finished) if current == op => {
                for row in &mut self.rows {
                    row.scan.get_or_insert(ScanResult::Stopped);
                }
                self.operation = Operation::Idle;
                if self.closing {
                    self.pending_clean = None;
                    None
                } else {
                    self.pending_clean
                        .take()
                        .map(|targets| self.begin_clean(targets))
                }
            }
            (Operation::Cleaning(current), Event::Cleaning(id)) if current == op => {
                self.row_mut(id).clean = Some(CleanProgress::Cleaning);
                None
            }
            (Operation::Cleaning(current), Event::Cleaned(id, result)) if current == op => {
                let row = self.row_mut(id);
                row.clean = Some(CleanProgress::Done(result.clone()));
                let target = row.target;
                self.clean_results.push((target, result));
                None
            }
            (Operation::Cleaning(current), Event::Finished) if current == op => {
                for row in &mut self.rows {
                    if matches!(
                        row.clean,
                        Some(CleanProgress::Queued | CleanProgress::Cleaning)
                    ) {
                        row.clean = None;
                    }
                }
                self.last_clean = Some(std::mem::take(&mut self.clean_results));
                self.operation = Operation::Idle;
                if self.closing {
                    None
                } else {
                    self.start_scan()
                }
            }
            _ => None,
        }
    }

    /// True when every selected, present Target has a complete estimate, at least
    /// one exists, and no Clean is running or pending.
    pub fn can_clean(&self) -> bool {
        if !self.clean_validated || self.selection_locked() || !self.selection_saved || self.closing
        {
            return false;
        }
        let mut any_complete = false;
        for row in self.rows.iter().filter(|row| row.selected) {
            match row.scan {
                Some(ScanResult::Complete { .. }) => any_complete = true,
                Some(ScanResult::NotPresent) => {}
                _ => return false,
            }
        }
        any_complete
    }

    /// Captures the ready Selection and starts a Clean, stopping an unfinished Scan first.
    pub fn request_clean(&mut self) -> Option<Command> {
        if !self.can_clean() {
            return None;
        }
        let targets = self
            .rows
            .iter()
            .filter(|row| row.selected && matches!(row.scan, Some(ScanResult::Complete { .. })))
            .map(|row| row.target.id)
            .collect();
        for row in &mut self.rows {
            row.clean = None;
        }
        match self.operation {
            Operation::Scanning(scan) => {
                self.pending_clean = Some(targets);
                Some(Command::Stop(scan))
            }
            _ => Some(self.begin_clean(targets)),
        }
    }

    /// True while a Clean is pending or running; the Selection cannot change.
    pub fn selection_locked(&self) -> bool {
        self.closing
            || self.pending_clean.is_some()
            || matches!(self.operation, Operation::Cleaning(_))
    }

    /// Flips a Target's selection. Rejected while the Selection is locked.
    pub fn toggle(&mut self, id: TargetId) -> bool {
        if self.selection_locked() {
            return false;
        }
        let Some(row) = self.rows.iter_mut().find(|row| row.target.id == id) else {
            return false;
        };
        row.selected = !row.selected;
        true
    }

    pub fn set_selection_saved(&mut self, saved: bool) {
        self.selection_saved = saved;
    }

    pub fn replace_selection(&mut self, choices: &Choices) {
        for row in &mut self.rows {
            row.selected = choices.get(row.target.id).copied().unwrap_or(false);
        }
    }

    /// Stop the worker and suppress a pending Clean or automatic rescan.
    pub fn close(&mut self) -> Option<Command> {
        self.closing = true;
        self.pending_clean = None;
        match self.operation {
            Operation::Scanning(op) | Operation::Cleaning(op) => Some(Command::Stop(op)),
            Operation::Idle => None,
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing
    }

    pub fn clean_validated(&self) -> bool {
        self.clean_validated
    }

    /// Per-Target results of the latest finished Clean, in Clean order.
    pub fn last_clean(&self) -> Option<&[(&'static Target, CleanResult)]> {
        self.last_clean.as_deref()
    }

    fn begin_clean(&mut self, targets: Vec<TargetId>) -> Command {
        let op = self.begin_op();
        self.operation = Operation::Cleaning(op);
        for &id in &targets {
            self.row_mut(id).clean = Some(CleanProgress::Queued);
        }
        Command::Clean { op, targets }
    }

    fn row_mut(&mut self, id: TargetId) -> &mut Row {
        self.rows
            .iter_mut()
            .find(|row| row.target.id == id)
            .expect("workers only report built-in Target IDs")
    }

    fn begin_op(&mut self) -> OpId {
        self.next_op += 1;
        OpId(self.next_op)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::{CleanStatus, Problem};

    fn scan_op(controller: &mut Controller) -> OpId {
        match controller.start_scan().unwrap() {
            Command::Scan { op, .. } => op,
            _ => unreachable!(),
        }
    }

    #[test]
    fn selected_targets_scan_first_and_incomplete_results_block_clean() {
        let mut controller = Controller::new(|target| target.id == "windows-temp", true);
        let Command::Scan { op, targets } = controller.start_scan().unwrap() else {
            unreachable!()
        };
        assert_eq!(targets, ["windows-temp", "user-temp"]);
        controller.apply(
            op,
            Event::Scanned(
                "windows-temp",
                ScanResult::Partial {
                    bytes: 5,
                    problem: Problem::AccessDenied,
                },
            ),
        );
        assert!(!controller.can_clean());
        controller.apply(op, Event::Finished);
        assert!(!controller.can_clean());
        assert!(controller.toggle("windows-temp"));
        assert!(controller.toggle("user-temp"));
        assert!(
            !controller.can_clean(),
            "an unscanned selected Target is not ready"
        );
    }

    #[test]
    fn clean_waits_for_unfinished_unticked_scan_then_rescans() {
        let mut controller = Controller::new(|target| target.id == "user-temp", true);
        let scan = scan_op(&mut controller);
        controller.apply(
            scan,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 10 }),
        );
        assert!(controller.can_clean());
        assert_eq!(controller.request_clean(), Some(Command::Stop(scan)));
        assert!(!controller.toggle("windows-temp"));
        assert_eq!(controller.request_clean(), None);
        let Some(Command::Clean { op, targets }) = controller.apply(scan, Event::Finished) else {
            panic!("expected Clean")
        };
        assert_eq!(targets, ["user-temp"]);
        let result = CleanResult {
            status: CleanStatus::Complete,
            deleted_bytes: 10,
            skipped: vec![],
            coverage_problem: None,
        };
        controller.apply(op, Event::Cleaned("user-temp", result.clone()));
        assert!(matches!(
            controller.apply(op, Event::Finished),
            Some(Command::Scan { .. })
        ));
        assert_eq!(controller.last_clean(), Some(&[(&TARGETS[0], result)][..]));
    }

    #[test]
    fn unsaved_selection_blocks_clean_and_close_suppresses_rescan() {
        let mut controller = Controller::new(|_| true, true);
        let scan = scan_op(&mut controller);
        controller.apply(
            scan,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 2 }),
        );
        controller.apply(scan, Event::Scanned("windows-temp", ScanResult::NotPresent));
        controller.set_selection_saved(false);
        assert!(!controller.can_clean());
        controller.set_selection_saved(true);
        assert!(controller.can_clean());
        controller.apply(scan, Event::Finished);
        let Some(Command::Clean { op, .. }) = controller.request_clean() else {
            panic!("expected Clean")
        };
        assert_eq!(controller.close(), Some(Command::Stop(op)));
        assert!(!controller.toggle("user-temp"));
        assert_eq!(controller.apply(op, Event::Finished), None);
        assert_eq!(controller.operation(), Operation::Idle);
    }

    #[test]
    fn late_results_cannot_overwrite_new_scan() {
        let mut controller = Controller::new(|_| false, true);
        let first = scan_op(&mut controller);
        controller.apply(first, Event::Finished);
        let second = scan_op(&mut controller);
        controller.apply(
            first,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 99 }),
        );
        assert!(controller.rows()[0].scan.is_none());
        assert_ne!(first, second);
    }

    #[test]
    fn duplicate_requests_do_not_start_another_operation() {
        let mut controller = Controller::new(|_| true, true);
        let scan = scan_op(&mut controller);
        assert!(controller.start_scan().is_none());
        controller.apply(
            scan,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 2 }),
        );
        controller.apply(scan, Event::Scanned("windows-temp", ScanResult::NotPresent));
        assert_eq!(controller.request_clean(), Some(Command::Stop(scan)));
        assert_eq!(controller.request_clean(), None);
        let Some(Command::Clean { op, targets }) = controller.apply(scan, Event::Finished) else {
            panic!("expected Clean after Scan completion")
        };
        assert_eq!(targets, ["user-temp"]);
        assert!(controller.start_scan().is_none());
        assert_eq!(controller.request_clean(), None);
        controller.apply(op, Event::Finished);
    }

    #[test]
    fn close_during_scan_cancels_pending_clean_after_worker_acknowledges_stop() {
        let mut controller = Controller::new(|_| true, true);
        let scan = scan_op(&mut controller);
        controller.apply(
            scan,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 2 }),
        );
        controller.apply(scan, Event::Scanned("windows-temp", ScanResult::NotPresent));
        assert_eq!(controller.request_clean(), Some(Command::Stop(scan)));
        assert_eq!(controller.close(), Some(Command::Stop(scan)));
        assert_eq!(controller.apply(scan, Event::Finished), None);
        assert_eq!(controller.operation(), Operation::Idle);
        assert_eq!(controller.request_clean(), None);
        assert_eq!(controller.start_scan(), None);
    }

    #[test]
    fn finished_clean_results_follow_worker_order_and_survive_rescan() {
        let mut controller = Controller::new(|_| true, true);
        let scan = scan_op(&mut controller);
        for id in ["user-temp", "windows-temp"] {
            controller.apply(scan, Event::Scanned(id, ScanResult::Complete { bytes: 1 }));
        }
        controller.apply(scan, Event::Finished);
        let Some(Command::Clean { op, .. }) = controller.request_clean() else {
            panic!("expected Clean")
        };
        for id in ["user-temp", "windows-temp"] {
            controller.apply(
                op,
                Event::Cleaned(
                    id,
                    CleanResult {
                        status: CleanStatus::Complete,
                        deleted_bytes: 1,
                        skipped: vec![],
                        coverage_problem: None,
                    },
                ),
            );
        }
        let Some(Command::Scan { op: rescan, .. }) = controller.apply(op, Event::Finished) else {
            panic!("expected rescan")
        };
        assert_eq!(
            controller
                .last_clean()
                .unwrap()
                .iter()
                .map(|(target, _)| target.id)
                .collect::<Vec<_>>(),
            ["user-temp", "windows-temp"]
        );
        controller.apply(rescan, Event::Scanned("user-temp", ScanResult::NotPresent));
        assert_eq!(controller.last_clean().unwrap().len(), 2);
    }

    #[test]
    fn unvalidated_clean_is_unavailable_even_when_scan_is_ready() {
        let mut controller = Controller::new(|target| target.id == "user-temp", false);
        let scan = scan_op(&mut controller);
        controller.apply(
            scan,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 2 }),
        );
        assert!(!controller.can_clean());
        assert_eq!(controller.request_clean(), None);
    }
}
