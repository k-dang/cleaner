//! Owns the Selection, Scan results, the current operation, and the last Clean
//! result. It enforces operation exclusion and Clean readiness independently of
//! the UI. Workers receive `Command`s and report back through `apply`; the
//! Selection store receives `Choices` and reports back through `save_finished`.

use std::collections::VecDeque;

use crate::results::{CleanResult, ScanResult};
use crate::selection::{self, Choices, Loaded};
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

/// A Selection handed to the store whose save has not reported yet.
struct PendingSave {
    choices: Choices,
    /// The rewrite of a loaded Selection, rather than a user change.
    initial: bool,
}

pub struct Controller {
    rows: Vec<Row>,
    operation: Operation,
    next_op: u64,
    /// Targets captured by a Clean request that waits for the Scan worker to stop.
    pending_clean: Option<Vec<TargetId>>,
    /// Results reported so far by the running Clean.
    clean_results: Vec<CleanResult>,
    last_clean: Option<Vec<CleanResult>>,
    selection_loaded: bool,
    /// Saves in request order; the store reports them in the same order.
    pending_saves: VecDeque<PendingSave>,
    /// The latest Selection known to be stored.
    saved_choices: Option<Choices>,
    selection_error: Option<String>,
    closing: bool,
}

impl Controller {
    /// Starts with nothing selected; the Selection arrives through `load_selection`.
    pub fn new() -> Self {
        let rows = TARGETS
            .iter()
            .map(|target| Row {
                target,
                selected: false,
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
            selection_loaded: false,
            pending_saves: VecDeque::new(),
            saved_choices: None,
            selection_error: None,
            closing: false,
        }
    }

    /// True when a Scan may start: the Selection is loaded and nothing runs.
    pub fn can_scan(&self) -> bool {
        self.selection_loaded && self.operation == Operation::Idle && !self.closing
    }

    /// Starts a Scan of every Target, selected ones first.
    pub fn start_scan(&mut self) -> Option<Command> {
        if !self.can_scan() {
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
                self.row_mut(id).clean = Some(CleanProgress::Done(result.clone()));
                self.clean_results.push(result);
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

    /// True when the Selection is saved and unlocked, every selected, present
    /// Target has a complete estimate, and at least one exists.
    pub fn can_clean(&self) -> bool {
        let saved = self.pending_saves.is_empty() && self.saved_choices.is_some();
        if self.selection_locked() || !saved {
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

    /// True until the Selection loads, and while closing or a Clean is pending
    /// or running; the Selection cannot change.
    pub fn selection_locked(&self) -> bool {
        !self.selection_loaded
            || self.closing
            || self.pending_clean.is_some()
            || matches!(self.operation, Operation::Cleaning(_))
    }

    /// Applies the stored Selection, or none on error. Returns the Choices to
    /// save when the stored file needs rewriting.
    pub fn load_selection(&mut self, loaded: Result<Loaded, String>) -> Option<Choices> {
        self.selection_loaded = true;
        match loaded {
            Ok(Loaded {
                choices,
                needs_save,
            }) => {
                self.replace_selection(&choices);
                if needs_save {
                    return Some(self.queue_save(choices, true));
                }
                self.saved_choices = Some(choices);
            }
            Err(error) => {
                self.replace_selection(&selection::choices(|_| false));
                self.selection_error = Some(format!("Selection could not be loaded: {error}"));
            }
        }
        None
    }

    /// Flips a Target's selection and returns the Choices to save. Rejected while
    /// the Selection is locked.
    pub fn toggle(&mut self, id: TargetId) -> Option<Choices> {
        if self.selection_locked() {
            return None;
        }
        let row = self.row_mut(id);
        row.selected = !row.selected;
        let choices = self
            .rows
            .iter()
            .map(|row| (row.target.id.to_string(), row.selected))
            .collect();
        Some(self.queue_save(choices, false))
    }

    /// Applies the store's report for the oldest pending save. When the last user
    /// change fails, the Selection returns to the latest stored one.
    pub fn save_finished(&mut self, result: Result<(), String>) {
        let pending = self
            .pending_saves
            .pop_front()
            .expect("every reported save was queued");
        match result {
            Ok(()) => {
                self.saved_choices = Some(pending.choices);
                if self.pending_saves.is_empty() {
                    self.selection_error = None;
                }
            }
            Err(error) => {
                self.selection_error = Some(format!("Selection could not be saved: {error}"));
                if self.pending_saves.is_empty() && !pending.initial {
                    let restored = self
                        .saved_choices
                        .clone()
                        .unwrap_or_else(|| selection::choices(|_| false));
                    self.replace_selection(&restored);
                }
            }
        }
    }

    pub fn selection_error(&self) -> Option<&str> {
        self.selection_error.as_deref()
    }

    pub fn is_selection_loaded(&self) -> bool {
        self.selection_loaded
    }

    pub fn is_saving(&self) -> bool {
        !self.pending_saves.is_empty()
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

    /// True once closing and no worker or save is still running.
    pub fn ready_to_exit(&self) -> bool {
        self.closing && self.operation == Operation::Idle && self.pending_saves.is_empty()
    }

    /// Per-Target results of the latest finished Clean, in Clean order.
    pub fn last_clean(&self) -> Option<&[CleanResult]> {
        self.last_clean.as_deref()
    }

    fn replace_selection(&mut self, choices: &Choices) {
        for row in &mut self.rows {
            row.selected = choices.get(row.target.id).copied().unwrap_or(false);
        }
    }

    fn queue_save(&mut self, choices: Choices, initial: bool) -> Choices {
        self.pending_saves.push_back(PendingSave {
            choices: choices.clone(),
            initial,
        });
        choices
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
            .expect("callers only pass built-in Target IDs")
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

    /// A controller whose stored Selection is already loaded and saved.
    fn loaded(pick: impl Fn(&Target) -> bool) -> Controller {
        let mut controller = Controller::new();
        let save = controller.load_selection(Ok(Loaded {
            choices: selection::choices(pick),
            needs_save: false,
        }));
        assert_eq!(save, None);
        controller
    }

    /// Picks only the two temp Targets, so tests can finish a Scan with two reports.
    fn temps(target: &Target) -> bool {
        matches!(target.id, "user-temp" | "windows-temp")
    }

    fn cleaned(deleted_bytes: u64) -> CleanResult {
        CleanResult {
            status: CleanStatus::Complete,
            deleted_bytes,
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
        let mut controller = loaded(|target| target.id == "windows-temp");
        let Command::Scan { op, targets } = controller.start_scan().unwrap() else {
            unreachable!()
        };
        assert_eq!(targets[..2], ["windows-temp", "user-temp"]);
        assert_eq!(targets.len(), TARGETS.len());
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
        assert!(controller.toggle("windows-temp").is_some());
        assert!(controller.toggle("user-temp").is_some());
        controller.save_finished(Ok(()));
        controller.save_finished(Ok(()));
        assert!(
            !controller.can_clean(),
            "an unscanned selected Target is not ready"
        );
    }

    #[test]
    fn clean_waits_for_unfinished_unticked_scan_then_rescans() {
        let mut controller = loaded(|target| target.id == "user-temp");
        let scan = scan_op(&mut controller);
        controller.apply(
            scan,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 10 }),
        );
        assert!(controller.can_clean());
        assert_eq!(controller.request_clean(), Some(Command::Stop(scan)));
        assert!(controller.toggle("windows-temp").is_none());
        assert_eq!(controller.request_clean(), None);
        let Some(Command::Clean { op, targets }) = controller.apply(scan, Event::Finished) else {
            panic!("expected Clean")
        };
        assert_eq!(targets, ["user-temp"]);
        controller.apply(op, Event::Cleaned("user-temp", cleaned(10)));
        assert!(matches!(
            controller.apply(op, Event::Finished),
            Some(Command::Scan { .. })
        ));
        assert_eq!(controller.last_clean(), Some(&[cleaned(10)][..]));
    }

    #[test]
    fn unsaved_selection_blocks_clean_and_close_suppresses_rescan() {
        let mut controller = loaded(temps);
        let scan = scan_op(&mut controller);
        controller.apply(
            scan,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 2 }),
        );
        controller.apply(scan, Event::Scanned("windows-temp", ScanResult::NotPresent));
        assert!(controller.toggle("windows-temp").is_some());
        assert!(!controller.can_clean(), "an unsaved Selection blocks Clean");
        controller.save_finished(Ok(()));
        assert!(controller.can_clean());
        controller.apply(scan, Event::Finished);
        let Some(Command::Clean { op, .. }) = controller.request_clean() else {
            panic!("expected Clean")
        };
        assert_eq!(controller.close(), Some(Command::Stop(op)));
        assert!(controller.toggle("user-temp").is_none());
        assert_eq!(controller.apply(op, Event::Finished), None);
        assert!(controller.ready_to_exit());
    }

    #[test]
    fn late_results_cannot_overwrite_new_scan() {
        let mut controller = loaded(|_| false);
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
        let mut controller = loaded(temps);
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
        let mut controller = loaded(temps);
        let scan = scan_op(&mut controller);
        controller.apply(
            scan,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 2 }),
        );
        controller.apply(scan, Event::Scanned("windows-temp", ScanResult::NotPresent));
        assert_eq!(controller.request_clean(), Some(Command::Stop(scan)));
        assert_eq!(controller.close(), Some(Command::Stop(scan)));
        assert_eq!(controller.apply(scan, Event::Finished), None);
        assert!(controller.ready_to_exit());
        assert_eq!(controller.request_clean(), None);
        assert_eq!(controller.start_scan(), None);
    }

    #[test]
    fn finished_clean_results_follow_worker_order_and_survive_rescan() {
        let mut controller = loaded(temps);
        let scan = scan_op(&mut controller);
        for id in ["user-temp", "windows-temp"] {
            controller.apply(scan, Event::Scanned(id, ScanResult::Complete { bytes: 1 }));
        }
        controller.apply(scan, Event::Finished);
        let Some(Command::Clean { op, .. }) = controller.request_clean() else {
            panic!("expected Clean")
        };
        controller.apply(op, Event::Cleaned("windows-temp", cleaned(2)));
        controller.apply(op, Event::Cleaned("user-temp", cleaned(1)));
        let Some(Command::Scan { op: rescan, .. }) = controller.apply(op, Event::Finished) else {
            panic!("expected rescan")
        };
        assert_eq!(controller.last_clean(), Some(&[cleaned(2), cleaned(1)][..]));
        controller.apply(rescan, Event::Scanned("user-temp", ScanResult::NotPresent));
        assert_eq!(controller.last_clean().unwrap().len(), 2);
    }

    #[test]
    fn selection_is_locked_until_loaded() {
        let mut controller = Controller::new();
        assert_eq!(controller.start_scan(), None);
        assert!(controller.toggle("user-temp").is_none());
        controller.load_selection(Err("unreadable".into()));
        assert!(controller.rows().iter().all(|row| !row.selected));
        assert!(controller.start_scan().is_some());
    }

    #[test]
    fn failed_save_restores_the_last_stored_selection() {
        let mut controller = loaded(temps);
        assert!(controller.toggle("user-temp").is_some());
        controller.save_finished(Err("disk full".into()));
        assert!(controller.rows()[0].selected);
        assert_eq!(
            controller.selection_error(),
            Some("Selection could not be saved: disk full")
        );
    }

    #[test]
    fn failed_initial_save_keeps_clean_disabled() {
        let mut controller = Controller::new();
        let save = controller.load_selection(Ok(Loaded {
            choices: selection::defaults(),
            needs_save: true,
        }));
        assert_eq!(save, Some(selection::defaults()));
        let scan = scan_op(&mut controller);
        controller.apply(
            scan,
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 2 }),
        );
        controller.apply(scan, Event::Scanned("windows-temp", ScanResult::NotPresent));
        controller.save_finished(Err("access denied".into()));
        assert!(
            controller.rows()[0].selected,
            "the loaded Selection stays shown"
        );
        assert!(!controller.can_clean());
    }
}
