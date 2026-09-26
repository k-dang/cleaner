# 02 - Complete the temp cleanup workflow

**What to build:** The complete launch, Scan, review, Clean, result, and rescan workflow for User temp and Windows temp. Kevin can preserve his Selection between runs and close the window to stop further cleanup. Follow the feature PRD's persistence, safety, result, and lifecycle contracts; keep the core and Selection store independent of GPUI.

**Blocked by:** 01 - Build and validate the portable GPUI app.

**Status:** ready-for-agent

- [ ] Validate both temp Targets in a disposable Windows environment before enabling their cleanup. Record location and owner-behavior evidence, including concurrent activity and interruption. Present the 24-hour Minimum age as a heuristic, not proof that a file is unused.
- [ ] Replace fixture actions with real Scan and Clean operations. Resolve built-in local roots through Windows APIs, accept only known Target IDs, and reject network locations before enumeration. Keep filesystem work on a worker thread with ordered UI updates.
- [ ] Scan automatically on launch, processing selected Targets first and reporting rows as they finish. Apply the Minimum age consistently to Scan and Clean, preserve files on the cutoff or with future timestamps, and expose unreadable metadata as incomplete coverage.
- [ ] Distinguish complete, absent, partial, failed, and stopped Scan results. Hide confirmed absent Targets while retaining their choices. Show incomplete estimates and access errors without treating them as zero.
- [ ] Persist explicit ticks and unticks by Target ID. Cover first-run defaults, new and obsolete IDs, temporarily absent Targets, serialized atomic replacement, rapid changes, and interruption. An unreadable or malformed store starts with all Targets unticked and an error. Save failures restore the last saved choices, or all unticked if none exist, and remain visible.
- [ ] Enable Clean only for a saved Selection whose selected Targets are complete or confirmed absent, with at least one complete present Target. Clean can stop unfinished unticked Scans and begin after the Scan worker acknowledges completion. Capture the ready Selection at the request and reject changes during handoff and Clean.
- [ ] Enforce one app instance per user and one active operation independently of disabled UI controls. Duplicate requests and late results cannot overlap operations or overwrite a newer operation's state.
- [ ] Permanently delete only eligible contents, retain Target roots, and recheck eligibility during Clean. Reject redirected roots and ancestors. Remove eligible inner links only as links, never follow their destinations, and skip unsupported reparse points.
- [ ] Preserve confinement through concurrent directory replacement using verified directory handles and suitable Windows operations. Deterministic race tests prove that outside sentinel trees are neither visited nor modified; a static junction test is insufficient.
- [ ] Skip rejected deletions without forcing permissions, attributes, ownership, app closure, or reboot deletion. Report sharing violations, access errors, and other failures distinctly. Test handles with and without delete sharing and files removed concurrently.
- [ ] Report known logical bytes accepted for deletion and known skip counts, with partial, failed, and stopped results. Unknown subtree counts remain unknown. Neither Scan differences nor successful delete requests are presented as exact reclaimed disk space.
- [ ] Show queued, active, and final row results; preserve the final result line during the automatic post-Clean Scan. Rescan works when idle, and failed deletions are not retried automatically.
- [ ] Normal close stops new filesystem work after the current step, suppresses automatic rescan, completes pending Selection saves, and exits after worker completion. Verify that completed deletions remain permanent and forced process termination leaves tolerable partial cleanup.
- [ ] Pass focused fixture tests for the above behavior, including age boundaries, missing and inaccessible content, root preservation, Selection persistence, readiness, operation exclusion, and shutdown. Use controlled completion rather than timing sleeps.
- [ ] Demonstrate the complete packaged workflow on seeded disposable data, checking both UI results and remaining files. Verify restart persistence and closing during Scan and Clean. Do not use Kevin's live temp folders as the first destructive test.
