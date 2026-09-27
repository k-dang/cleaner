//! Owns the Selection, Scan results, the current operation, and the last Clean
//! result. It enforces operation exclusion and Clean readiness independently of
//! the UI. Workers receive `Command`s and report back through `apply`.

use crate::results::{CleanResult, ScanResult};
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
}

impl Controller {
    pub fn new(is_selected: impl Fn(&Target) -> bool) -> Self {
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
        }
    }

    /// Starts a Scan of every Target, selected ones first. Only allowed when idle.
    pub fn start_scan(&mut self) -> Option<Command> {
        if self.operation != Operation::Idle {
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

    /// A Target's complete estimate from the latest Scan, if it has one.
    pub fn estimate(&self, id: TargetId) -> Option<u64> {
        match self.row(id).scan {
            Some(ScanResult::Complete { bytes }) => Some(bytes),
            _ => None,
        }
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
                self.pending_clean
                    .take()
                    .map(|targets| self.begin_clean(targets))
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
                self.start_scan()
            }
            _ => None,
        }
    }

    /// True when every selected, present Target has a complete estimate, at least
    /// one exists, and no Clean is running or pending.
    pub fn can_clean(&self) -> bool {
        if self.selection_locked() {
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
        self.pending_clean.is_some() || matches!(self.operation, Operation::Cleaning(_))
    }

    /// Flips a Target's selection. Rejected while the Selection is locked.
    pub fn toggle(&mut self, id: TargetId) -> bool {
        if self.selection_locked() {
            return false;
        }
        let row = self.row_mut(id);
        row.selected = !row.selected;
        true
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

    fn row(&self, id: TargetId) -> &Row {
        self.rows
            .iter()
            .find(|row| row.target.id == id)
            .expect("callers only pass built-in Target IDs")
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

    fn selecting(ids: &'static [TargetId]) -> Controller {
        Controller::new(|t| ids.contains(&t.id))
    }

    fn scanned_targets(command: Option<Command>) -> Vec<TargetId> {
        match command {
            Some(Command::Scan { targets, .. }) => targets,
            other => panic!("expected a Scan, got {other:?}"),
        }
    }

    #[test]
    fn scan_visits_selected_targets_first_then_the_rest_in_table_order() {
        let mut c = selecting(&["npm-cache", "user-temp"]);
        let order = scanned_targets(c.start_scan());
        assert_eq!(&order[..3], ["user-temp", "npm-cache", "windows-temp"]);
        assert_eq!(order.len(), TARGETS.len());
    }

    fn scan_op(c: &mut Controller) -> OpId {
        match c.start_scan() {
            Some(Command::Scan { op, .. }) => op,
            other => panic!("expected a Scan, got {other:?}"),
        }
    }

    fn scan_result(c: &Controller, id: TargetId) -> Option<ScanResult> {
        c.rows()
            .iter()
            .find(|r| r.target.id == id)
            .unwrap()
            .scan
            .clone()
    }

    #[test]
    fn totals_count_only_complete_estimates() {
        let mut c = selecting(&["user-temp", "npm-cache"]);
        let op = scan_op(&mut c);
        c.apply(
            op,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 100 }),
        );
        c.apply(
            op,
            Event::Scanned(
                "npm-cache",
                ScanResult::Partial {
                    bytes: 50,
                    problem: Problem::AccessDenied,
                },
            ),
        );
        c.apply(
            op,
            Event::Scanned("windows-temp", ScanResult::Complete { bytes: 30 }),
        );

        let totals = c.totals();
        assert_eq!(totals.complete_bytes, 130);
        assert_eq!(totals.selected_bytes, 100);
        assert_eq!(totals.incomplete, 1);
        assert!(totals.scanning);
    }

    #[test]
    fn targets_that_never_reported_are_stopped_when_the_scan_finishes() {
        let mut c = selecting(&["user-temp"]);
        let op = scan_op(&mut c);
        c.apply(
            op,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 1 }),
        );
        c.apply(op, Event::Finished);

        assert_eq!(c.operation(), Operation::Idle);
        assert_eq!(scan_result(&c, "npm-cache"), Some(ScanResult::Stopped));
        assert!(!c.totals().scanning);
    }

    #[test]
    fn results_from_a_finished_operation_cannot_overwrite_the_next_one() {
        let mut c = selecting(&["user-temp"]);
        let old = scan_op(&mut c);
        c.apply(old, Event::Finished);
        let _new = scan_op(&mut c);

        c.apply(
            old,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 1 }),
        );
        c.apply(old, Event::Finished);

        assert_eq!(scan_result(&c, "user-temp"), None);
        assert!(c.totals().scanning);
    }

    fn complete(bytes: u64) -> ScanResult {
        ScanResult::Complete { bytes }
    }

    fn done(bytes: u64) -> CleanResult {
        CleanResult {
            status: CleanStatus::Complete,
            deleted_bytes: Some(bytes),
            skipped: vec![],
            coverage_problem: None,
        }
    }

    fn clean_progress(c: &Controller, id: TargetId) -> Option<CleanProgress> {
        c.rows()
            .iter()
            .find(|r| r.target.id == id)
            .unwrap()
            .clean
            .clone()
    }

    #[test]
    fn clean_waits_for_selected_targets_but_not_for_unticked_ones() {
        let mut c = selecting(&["user-temp", "brave-cache"]);
        let op = scan_op(&mut c);
        c.apply(op, Event::Scanned("user-temp", complete(10)));
        assert!(!c.can_clean());

        c.apply(op, Event::Scanned("brave-cache", ScanResult::NotPresent));
        assert!(
            c.can_clean(),
            "absent Targets do not block, unticked ones are still scanning"
        );
    }

    #[test]
    fn clean_needs_a_complete_present_target() {
        let mut c = selecting(&["brave-cache"]);
        let op = scan_op(&mut c);
        c.apply(op, Event::Scanned("brave-cache", ScanResult::NotPresent));
        c.apply(op, Event::Finished);
        assert!(!c.can_clean());
    }

    #[test]
    fn a_selected_partial_result_blocks_clean_until_unticked() {
        let mut c = selecting(&["user-temp", "npm-cache"]);
        let op = scan_op(&mut c);
        c.apply(op, Event::Scanned("user-temp", complete(10)));
        c.apply(
            op,
            Event::Scanned(
                "npm-cache",
                ScanResult::Failed {
                    problem: Problem::AccessDenied,
                },
            ),
        );
        c.apply(op, Event::Finished);
        assert!(!c.can_clean());
        assert_eq!(c.request_clean(), None);

        assert!(c.toggle("npm-cache"));
        assert!(c.can_clean());
    }

    #[test]
    fn clean_captures_ready_selected_targets_when_idle() {
        let mut c = selecting(&["user-temp", "windows-temp", "brave-cache"]);
        let op = scan_op(&mut c);
        c.apply(op, Event::Scanned("user-temp", complete(10)));
        c.apply(op, Event::Scanned("windows-temp", complete(20)));
        c.apply(op, Event::Scanned("brave-cache", ScanResult::NotPresent));
        c.apply(op, Event::Finished);

        let Some(Command::Clean { targets, .. }) = c.request_clean() else {
            panic!("expected a Clean");
        };
        assert_eq!(targets, ["user-temp", "windows-temp"]);
        assert_eq!(clean_progress(&c, "user-temp"), Some(CleanProgress::Queued));
        assert_eq!(c.request_clean(), None, "duplicate request");
        assert!(!c.toggle("user-temp"), "Selection is locked during Clean");
        assert_eq!(c.start_scan(), None);
    }

    #[test]
    fn clean_stops_an_unfinished_scan_and_starts_after_the_worker_acknowledges() {
        let mut c = selecting(&["user-temp"]);
        let scan = scan_op(&mut c);
        c.apply(scan, Event::Scanned("user-temp", complete(10)));

        assert_eq!(c.request_clean(), Some(Command::Stop(scan)));
        assert!(!c.can_clean());
        assert!(!c.toggle("npm-cache"), "Selection is locked during handoff");
        assert_eq!(c.request_clean(), None, "duplicate request during handoff");

        let Some(Command::Clean { targets, .. }) = c.apply(scan, Event::Finished) else {
            panic!("expected the captured Clean to start");
        };
        assert_eq!(targets, ["user-temp"]);
        assert_eq!(scan_result(&c, "npm-cache"), Some(ScanResult::Stopped));
    }

    #[test]
    fn a_finished_clean_keeps_its_results_and_starts_a_fresh_scan() {
        let mut c = selecting(&["user-temp"]);
        let scan = scan_op(&mut c);
        c.apply(scan, Event::Scanned("user-temp", complete(10)));
        c.apply(scan, Event::Finished);
        let Some(Command::Clean { op, .. }) = c.request_clean() else {
            panic!("expected a Clean");
        };

        c.apply(op, Event::Cleaning("user-temp"));
        assert_eq!(
            clean_progress(&c, "user-temp"),
            Some(CleanProgress::Cleaning)
        );
        c.apply(op, Event::Cleaned("user-temp", done(10)));
        let rescan = c.apply(op, Event::Finished);

        assert!(matches!(rescan, Some(Command::Scan { .. })));
        assert_eq!(c.last_clean(), Some(&[(&TARGETS[0], done(10))][..]));
        assert_eq!(
            clean_progress(&c, "user-temp"),
            Some(CleanProgress::Done(done(10)))
        );
        assert!(!c.can_clean(), "the fresh Scan has not reported yet");
    }

    #[test]
    fn a_new_clean_clears_previous_row_results_before_stopping_the_rescan() {
        let mut c = selecting(&["user-temp", "pnpm-store"]);
        let mut fixture = crate::fixture::Fixture::default();
        let scan = scan_op(&mut c);
        for id in ["user-temp", "pnpm-store"] {
            c.apply(scan, Event::Scanned(id, fixture.scan(id).1));
        }
        c.apply(scan, Event::Finished);
        let Some(Command::Clean { op, targets }) = c.request_clean() else {
            panic!("expected a Clean");
        };
        for id in targets {
            c.apply(op, Event::Cleaning(id));
            let result = fixture.clean(id, c.estimate(id).unwrap()).1;
            c.apply(op, Event::Cleaned(id, result));
        }
        let Some(Command::Scan { op: rescan, .. }) = c.apply(op, Event::Finished) else {
            panic!("expected the automatic rescan");
        };
        c.apply(
            rescan,
            Event::Scanned("user-temp", fixture.scan("user-temp").1),
        );
        assert!(c.toggle("pnpm-store"));

        assert_eq!(c.request_clean(), Some(Command::Stop(rescan)));
        assert!(c.rows().iter().all(|row| row.clean.is_none()));

        let Some(Command::Clean { op, targets }) = c.apply(rescan, Event::Finished) else {
            panic!("expected the next Clean");
        };
        assert_eq!(targets, ["user-temp"]);
        assert_eq!(clean_progress(&c, "user-temp"), Some(CleanProgress::Queued));
        assert_eq!(clean_progress(&c, "pnpm-store"), None);
        assert_eq!(c.rows().iter().filter(|row| row.clean.is_some()).count(), 1);

        let result = fixture
            .clean("user-temp", c.estimate("user-temp").unwrap())
            .1;
        c.apply(op, Event::Cleaned("user-temp", result.clone()));
        c.apply(op, Event::Finished);
        assert_eq!(c.last_clean(), Some(&[(&TARGETS[0], result)][..]));
    }

    #[test]
    fn a_second_scan_is_rejected_while_one_is_running() {
        let mut c = selecting(&["user-temp"]);
        c.start_scan().unwrap();
        assert_eq!(c.start_scan(), None);
    }
}
