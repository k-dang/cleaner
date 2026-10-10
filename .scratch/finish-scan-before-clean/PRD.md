Status: implemented

# Finish Scan before Clean

## Problem Statement

Kevin finds the app's implementation complicated for a small, single-screen cleaner. One source of complexity is allowing Clean to start while unticked Targets are still being scanned. The app must capture a pending Selection, request that the Scan worker stop, wait for its completion report, lock the Selection during that handoff, and then dispatch Clean. The UI also explains this intermediate state.

This coordination makes the workflow harder to understand and maintain. The first simplification should remove this behavior and its supporting machinery while preserving deletion protections and the documented portable-core architecture.

## Solution

Use a straightforward sequence: Scan finishes, the user reviews the Selection, and Clean starts on an explicit user action. Clean stays disabled for the entire Scan, including the automatic Scan after Clean. A Clean action attempted during Scan is ignored; it never queues work or stops Scan.

Scan continues to report each Target as it finishes, with selected Targets first. The user can still change the Selection while Scan runs, and each change is saved as it is today. Once Scan finishes, Clean becomes available only when the current Selection satisfies the existing readiness rules. Problems in unticked Targets do not block Clean after Scan finishes.

The tradeoff is deliberate: users wait for all Targets to finish scanning even if every selected Target already has a complete estimate.

## User Stories

1. As a user, I want Scan to finish before Clean becomes available, so that the sequence of operations is predictable.
2. As a user, I want Clean to remain disabled when selected Targets finish before unticked Targets, so that readiness consistently means Scan has finished.
3. As a user, I want an attempted Clean during Scan to leave Scan running, so that all Targets receive their results.
4. As a user, I want an attempted Clean during Scan to be ignored, so that deletion cannot start later without another action from me.
5. As a user, I want Scan completion to leave the app ready for review, so that completion alone never starts Clean.
6. As a user, I want each Target's result to appear as it finishes, so that I can follow progress without waiting for the whole Scan.
7. As a user, I want selected Targets scanned first, so that useful estimates appear early.
8. As a user, I want to change the Selection during Scan, so that I can review the checklist while results arrive.
9. As a user, I want each Selection change remembered automatically, so that this simplification preserves my saved choices.
10. As a user, I want Clean blocked while a Selection save is pending, so that deletion uses a Selection that has been saved successfully.
11. As a user, I want existing Selection load and save errors to remain visible, so that persistence failures are understandable.
12. As a user, I want selected Targets with partial, failed, stopped, or missing Scan results to block Clean, so that incomplete inspection is never treated as readiness.
13. As a user, I want an incomplete unticked Target to leave Clean available after Scan finishes, so that an unrelated problem does not prevent cleaning my Selection.
14. As a user, I want absent selected Targets omitted from Clean, so that missing folders are treated as normal.
15. As a user, I want Clean disabled when no selected Target has a complete estimate, so that an empty or entirely absent Selection cannot start work.
16. As a user, I want a complete zero-byte Target to retain its existing Clean eligibility, so that procedures such as emptying the Recycle Bin keep their current behavior.
17. As a user, I want Clean to capture my Selection when I start it, so that its contents remain stable throughout deletion.
18. As a user, I want the Selection locked during Clean, so that I cannot change the meaning of an operation already in progress.
19. As a user, I want Clean to use the latest Scan snapshots, so that its native cleanup procedures retain their existing constraints.
20. As a user, I want a new Scan to invalidate earlier readiness, so that old results cannot enable Clean during a rescan.
21. As a user, I want Clean results and the automatic rescan preserved, so that I can see what happened and review the remaining eligible content.
22. As a user, I want closing the app to stop ongoing work safely, so that simplifying the handoff does not weaken shutdown behavior.
23. As a keyboard or assistive-technology user, I want Clean's disabled state to match its behavior, so that unavailable actions are consistently communicated.
24. As a user, I want progress and status changes to preserve checklist layout and scroll position, so that the simpler workflow remains comfortable to use.
25. As a maintainer, I want the pending Clean handoff deleted completely, so that there is one fewer state and ordering requirement to reason about.
26. As a maintainer, I want focused tests through existing interfaces, so that behavior is verified without adding new architecture.

## Implementation Decisions

- Modify the portable workflow controller, the Windows UI's handoff-specific presentation, affected tests, and user-facing workflow documentation.
- Keep the two-package architecture established by the portable-core ADR. Workflow coordination remains in the portable core; Windows cleanup procedures, storage, appearance, startup, and GPUI remain in the native app.
- Require the controller to be idle before `can_clean` can return true. Preserve the existing Selection-loaded, saved-Selection, closing, and selected-Target readiness checks.
- Treat the current Scan's `Finished` report as the transition back to idle. Receiving the final per-Target result alone does not enable Clean.
- Make `request_clean` return no command while Scan runs. It must not capture a pending Selection, emit a Stop command, lock the Selection, or schedule a future Clean.
- After a completed Scan, an eligible `request_clean` immediately returns the Clean command with the captured selected, complete Targets and their current Scan snapshots.
- On Scan completion, update missing results and presentation as today, then return to idle without dispatching Clean. An explicit subsequent Clean action is required.
- Delete pending-Clean state and all logic dedicated to stopping Scan in order to start Clean. Remove obsolete handoff comments, status text, documentation, and tests; retain no shims, configuration switches, or dormant reintroduction hooks.
- Retain operation IDs and rejection of late reports. Retain Stop commands and cooperative cancellation for closing the app during Scan or Clean.
- Retain selected-first Scan order, per-Target progress, previous estimates shown during rescans, and current estimate formatting. Changing the Selection does not require dynamically reordering an already running Scan.
- Retain automatic Selection saving, queued-save behavior, rollback semantics, malformed-data handling, and the stored Selection format.
- Retain captured Selection semantics, Clean progress and results, the automatic post-Clean Scan, and suppression of that rescan when closing.
- Use the controller's readiness result for the Clean control's interaction, disabled styling, keyboard availability, and accessibility state. Remove the status message about stopping Scan to start Clean. Existing Scan progress communicates why Clean is unavailable; no additional dialog or notification is required.
- Preserve the Recycle Bin's latest covered-volume snapshot, handle-based traversal, link protections, Minimum age checks, locked-file handling, and target-folder preservation. These safeguards are outside the simplification.
- Do not introduce a new public interface, worker framework, scheduler, or generalized state machine for this change. Simplification is demonstrated by removing the handoff machinery.

## Testing Decisions

- Use the existing portable controller interface as the primary automated seam. Drive Selection loading, saves, Scan reports, Clean requests, and close requests; assert returned commands and observable readiness, Selection, and result behavior. Avoid assertions about private fields, collection choices, or exact implementation structure.
- Use existing controller tests as prior art for operation exclusion, selected-first ordering, Scan snapshot handoff, stale-report rejection, persistence failures, shutdown, and automatic rescanning. Update affected scenarios so a Scan finishes before a valid Clean request. Replace handoff tests with focused tests of the new behavior rather than keeping tests solely to prove deleted code is absent.
- Cover the central sequence: a selected Target reports a complete estimate while an unticked Target is still pending; Clean is unavailable and requesting it emits no command; Selection changes remain possible; Scan finishes without emitting Clean; a fresh eligible request emits Clean directly.
- Also cover the moment after every per-Target report but before `Finished`: Clean remains unavailable until the worker reports completion.
- Cover readiness after completion with one compact set of scenarios: an unticked failure permits Clean, a selected incomplete result blocks it, an absent Target is omitted, an entirely absent or empty Selection cannot Clean, and a complete zero-byte Target remains eligible.
- Keep focused coverage for current snapshots, rejection of stale reports, pending saves, duplicate requests, close during Scan or Clean, and the automatic rescan. During the automatic rescan, ready selected results alone must not re-enable Clean.
- Use the existing GPUI test window as the UI seam. Drive the controller with fixture reports and render the view to verify that Clean stays disabled during Scan and becomes available after completion when eligible. Preserve the existing checks for stable checklist bounds and scroll position; do not add a separate UI automation framework or a production fixture mode.
- Inspect the Windows UI for disabled styling, keyboard behavior, accessible disabled state, and the absence of handoff text. Use existing scratch-folder cleanup fixtures for any deletion exercised during verification. Do not invoke Clean against the user's real caches, Recycle Bin, or system folders.
- Retain the existing native cleanup checks as safety coverage. Add native cleanup tests only if a change to native behavior becomes necessary; that would be a scope change requiring renewed review.
- During implementation, run formatting and the portable-core tests and Clippy checks, then the Windows workspace tests and Clippy checks required by existing CI. Resolve observed failures and report any unavailable verification explicitly.

## Out of Scope

- Coalescing Selection saves or changing persistence and rollback semantics.
- Consolidating presentation summaries, removing unrelated fields, or broadly reorganizing the UI.
- Replacing GPUI, custom checkboxes, focus rings, or scrollbar behavior.
- Undoing the portable-core extraction or adding another operating system's native app.
- Changing the Target catalog, Default selection, cleanup eligibility, Minimum age, deletion safeguards, or Recycle Bin procedure.
- Adding pause, cancel, resume, queued Clean, confirmation dialogs, new settings, or new persistence formats.
- Executing destructive verification against real user data, changing production resources, releasing, or publishing the app.

## Further Notes

This spec synthesizes the first recommendation from the complexity assessment. Kevin requested a spec; the broader Selection-saving and UI simplifications remain separate proposals. The intended implementation is limited to finishing Scan before Clean and removing the resulting unnecessary handoff.

Kevin confirmed the existing testing seams: focused tests through the controller interface plus a Windows UI check using scratch Targets. Use the existing fixture facilities described above; do not introduce a new production testing mode.

Completion requires both observable behavior and actual deletion of the handoff machinery. A disabled button layered over the existing pending-Clean mechanism does not satisfy the simplification.

## Comments

- 2026-10-10: Implemented the Scan-completion gate, removed pending-Clean coordination and handoff presentation, and updated workflow documentation and the network-check script. Selection persistence and native deletion procedures retain their existing behavior.
- Verification: the central controller test failed against the original behavior and passed after the change. Windows workspace type-checking, formatting, Clippy with warnings denied, and all 75 workspace tests passed. The rendered fixture UI test verifies that Clean is skipped in keyboard navigation during Scan and becomes focusable after completion; existing layout checks also passed.
- Review: separate Standards and Spec reviews found only obsolete handoff wording in the network-check script. Both the header comment and final report have been corrected.
- Live Windows verification: after opening Codex and restarting T3, Computer Use connected successfully. Inspected the debug app's appearance and accessibility tree before and during Scan. Clean stayed visually and accessibly disabled after both selected Targets finished while unticked Targets were still scanning, and keyboard navigation skipped Clean while allowing access to the checklist. Closed the app during its lengthy pnpm Scan and verified that it exited. No Clean was invoked against real user data.
- Remaining verification limit: the live Scan was stopped before all Targets finished, so the transition to enabled Clean after completion remains covered by the controller and rendered fixture UI tests rather than a completed live Scan.
