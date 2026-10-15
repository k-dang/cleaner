//! Owns the Selection, Scan results, the current operation, and the last Clean
//! result. It enforces operation exclusion and Clean readiness independently of
//! the UI. Workers receive `Command`s and report back through `apply`; the
//! Selection store receives `Choices` and reports back through `save_finished`.

use std::collections::BTreeMap;

use crate::results::{CleanResult, Event, ScanResult};
use crate::selection::{self, Choices, Loaded};
use crate::targets::{Target, TargetId};

/// Identifies one Scan or Clean, so late reports from an earlier one are ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpId(u64);

/// Work the controller asks a worker to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Command<Snapshot> {
    Scan {
        op: OpId,
        targets: Vec<TargetId>,
    },
    Clean {
        op: OpId,
        targets: Vec<CleanTarget<Snapshot>>,
    },
    /// Ask the worker running `op` to stop between filesystem steps or shell calls.
    Stop(OpId),
}

/// A ready Target paired with the evidence from its latest Scan.
/// Only the native implementation interprets the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanTarget<Snapshot> {
    pub id: TargetId,
    pub snapshot: Snapshot,
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

impl Row {
    /// Absent Targets leave the checklist once their previous Clean result is replaced.
    pub fn is_visible(&self) -> bool {
        self.scan != Some(ScanResult::NotPresent) || self.clean.is_some()
    }
}

/// Where a Target is in the running or latest Clean.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanProgress {
    Queued,
    Cleaning,
    Done(CleanResult),
}

/// Current Scan estimates and Scan/Clean progress.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Sum of complete estimates; partial and unknown sizes are excluded.
    pub complete_bytes: u64,
    /// Complete selected bytes; `None` while selected results are pending
    /// and none has a complete estimate.
    pub selected_bytes: Option<u64>,
    /// At least one visible selected Target is still waiting for its Scan result.
    pub selected_pending: bool,
    /// Targets whose latest Scan is partial, failed, or stopped.
    pub incomplete: usize,
    pub scanning: bool,
    /// Complete estimates larger than zero, selected or not.
    pub found_targets: usize,
    /// Selected Targets currently shown in the checklist.
    pub selected_targets: usize,
    pub scanned_targets: usize,
    pub total_targets: usize,
    pub cleaned_targets: usize,
    pub clean_targets: usize,
}

pub struct Controller<Snapshot> {
    rows: Vec<Row>,
    operation: Operation,
    next_op: u64,
    /// Native evidence is tied to a Target and accepted only from the current Scan.
    snapshots: BTreeMap<TargetId, Snapshot>,
    /// The Target the running Scan is inspecting.
    scanning_target: Option<TargetId>,
    /// Results reported so far by the running Clean.
    clean_results: Vec<CleanResult>,
    last_clean: Option<Vec<CleanResult>>,
    selection_loaded: bool,
    /// Queued writes still awaiting a report from the ordered Selection store.
    pending_saves: usize,
    selection_error: Option<String>,
    closing: bool,
}

impl<Snapshot: Clone> Controller<Snapshot> {
    /// Starts with nothing selected; the Selection arrives through `load_selection`.
    /// The native catalog supplies unique, stable IDs in checklist order. Worker
    /// reports and user actions must name only Targets from that catalog.
    pub fn new(targets: impl IntoIterator<Item = &'static Target>) -> Self {
        let rows = targets
            .into_iter()
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
            snapshots: BTreeMap::new(),
            scanning_target: None,
            clean_results: Vec::new(),
            last_clean: None,
            selection_loaded: false,
            pending_saves: 0,
            selection_error: None,
            closing: false,
        }
    }

    /// True when a Scan may start: the Selection is loaded and nothing runs.
    pub fn can_scan(&self) -> bool {
        self.selection_loaded && self.operation == Operation::Idle && !self.closing
    }

    /// Starts a Scan of every Target, selected ones first.
    pub fn start_scan(&mut self) -> Option<Command<Snapshot>> {
        if !self.can_scan() {
            return None;
        }
        let op = self.begin_op();
        self.operation = Operation::Scanning(op);
        self.snapshots.clear();
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

    pub fn summary(&self) -> Summary {
        let mut summary = Summary {
            scanning: matches!(self.operation, Operation::Scanning(_)),
            total_targets: self.rows.len(),
            ..Summary::default()
        };
        let mut selected_bytes = 0;
        let mut selected_known = false;
        for row in &self.rows {
            summary.scanned_targets += usize::from(row.scan.is_some());
            summary.clean_targets += usize::from(row.clean.is_some());
            summary.cleaned_targets +=
                usize::from(matches!(row.clean, Some(CleanProgress::Done(_))));
            if row.selected && row.is_visible() {
                summary.selected_targets += 1;
                summary.selected_pending |= row.scan.is_none();
            }
            match row.scan {
                Some(ScanResult::Complete { bytes }) => {
                    summary.complete_bytes += bytes;
                    summary.found_targets += usize::from(bytes > 0);
                    if row.selected {
                        selected_bytes += bytes;
                        selected_known = true;
                    }
                }
                Some(
                    ScanResult::Partial { .. } | ScanResult::Failed { .. } | ScanResult::Stopped,
                ) => summary.incomplete += 1,
                Some(ScanResult::NotPresent) | None => {}
            }
        }
        summary.selected_pending &= summary.scanning;
        summary.selected_bytes =
            (selected_known || !summary.selected_pending).then_some(selected_bytes);
        summary
    }

    /// The Target the running Scan is inspecting, if any.
    pub fn scanning_target(&self) -> Option<TargetId> {
        self.scanning_target
    }

    /// Applies a worker report. Reports from any operation but the current one are ignored.
    pub fn apply(&mut self, op: OpId, event: Event<Snapshot>) -> Option<Command<Snapshot>> {
        match (self.operation, event) {
            (Operation::Scanning(current), Event::Scanning(id)) if current == op => {
                self.scanning_target = Some(id);
                None
            }
            (Operation::Scanning(current), Event::Scanned(id, result, snapshot))
                if current == op =>
            {
                self.set_scan(id, result);
                self.snapshots.insert(id, snapshot);
                None
            }
            (Operation::Scanning(current), Event::Finished) if current == op => {
                for row in &mut self.rows {
                    row.scan.get_or_insert(ScanResult::Stopped);
                }
                self.scanning_target = None;
                self.operation = Operation::Idle;
                None
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

    /// True when idle, the Selection is unlocked, every selected,
    /// present Target has a complete estimate, and at least one exists.
    pub fn can_clean(&self) -> bool {
        if self.operation != Operation::Idle || self.selection_locked() {
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

    /// Captures the ready Selection and starts a Clean. Rejected until Scan finishes.
    pub fn request_clean(&mut self) -> Option<Command<Snapshot>> {
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
        Some(self.begin_clean(targets))
    }

    /// True until the Selection loads, and while closing or Clean runs;
    /// the Selection cannot change.
    pub fn selection_locked(&self) -> bool {
        !self.selection_loaded || self.closing || matches!(self.operation, Operation::Cleaning(_))
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
                    return Some(self.begin_save(choices));
                }
            }
            Err(error) => {
                self.replace_selection(&selection::choices(
                    self.rows.iter().map(|row| row.target),
                    |_| false,
                ));
                self.selection_error = Some(format!("Selection could not be loaded: {error}"));
            }
        }
        None
    }

    /// Flips a Target unless the Selection is locked and queues the current
    /// choices for the store to save in order.
    pub fn toggle(&mut self, id: TargetId) -> Option<Choices> {
        if self.selection_locked() {
            return None;
        }
        let row = self.row_mut(id);
        row.selected = !row.selected;
        Some(self.begin_save(self.choices()))
    }

    /// Records an ordered save's outcome without changing the visible Selection.
    pub fn save_finished(&mut self, result: Result<(), String>) {
        self.pending_saves -= 1;
        self.selection_error = result
            .err()
            .map(|error| format!("Selection could not be saved: {error}"));
    }

    pub fn selection_error(&self) -> Option<&str> {
        self.selection_error.as_deref()
    }

    pub fn is_selection_loaded(&self) -> bool {
        self.selection_loaded
    }

    /// Stop the worker and suppress the automatic rescan.
    pub fn close(&mut self) -> Option<Command<Snapshot>> {
        self.closing = true;
        match self.operation {
            Operation::Scanning(op) | Operation::Cleaning(op) => Some(Command::Stop(op)),
            Operation::Idle => None,
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing
    }

    /// True once closing, Selection loading is finished, and no worker or save runs.
    pub fn ready_to_exit(&self) -> bool {
        self.closing
            && self.selection_loaded
            && self.operation == Operation::Idle
            && self.pending_saves == 0
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

    fn choices(&self) -> Choices {
        self.rows
            .iter()
            .map(|row| (row.target.id.to_string(), row.selected))
            .collect()
    }

    fn begin_save(&mut self, choices: Choices) -> Choices {
        self.pending_saves += 1;
        choices
    }

    /// Every requested Target has a complete current result and its native snapshot.
    fn begin_clean(&mut self, targets: Vec<TargetId>) -> Command<Snapshot> {
        let op = self.begin_op();
        self.operation = Operation::Cleaning(op);
        for &id in &targets {
            self.row_mut(id).clean = Some(CleanProgress::Queued);
        }
        Command::Clean {
            op,
            targets: targets
                .into_iter()
                .map(|id| CleanTarget {
                    id,
                    snapshot: self
                        .snapshots
                        .get(&id)
                        .expect("ready Targets have a current Scan snapshot")
                        .clone(),
                })
                .collect(),
        }
    }

    fn set_scan(&mut self, id: TargetId, result: ScanResult) {
        self.scanning_target = None;
        let row = self.row_mut(id);
        row.scan = Some(result);
        row.clean = None;
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
mod tests;
