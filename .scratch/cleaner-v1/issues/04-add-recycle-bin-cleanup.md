# 04 - Add Recycle Bin cleanup

**What to build:** A Recycle Bin Target that Kevin can select and empty in the same workflow as folder Targets. Use Windows shell operations and show their actual reporting limits. Follow the feature PRD's drive-scope, result, and shutdown contracts.

**Blocked by:** 02 - Complete the temp cleanup workflow.

**Status:** implemented - Kevin confirmed on 2026-09-30 that emptying a real Recycle Bin works; the other real-bin checks were not run

- [x] Show the Recycle Bin under Windows with its Default selection and a shell-provided Scan estimate. An empty bin remains a visible, complete Target, and its saved choice survives restart.
- [x] Enumerate the current account's mounted local fixed drives at Scan time. Query and Clean the same captured drive scope; do not silently broaden cleanup to drives that appeared later.
- [x] Query and empty through the Windows shell API, suppressing confirmation, progress UI, and sound. Never traverse the Recycle Bin filesystem directly. Check each operation result and represent partial drive failures accurately.
- [x] Integrate the Target with Selection capture, readiness, operation exclusion, row progress, the final result line, and automatic rescan. A mixed Clean can report ordinary deleted file sizes alongside the Recycle Bin outcome.
- [x] Deleted bytes and per-file skip counts for the Recycle Bin remain unavailable, including on success. Do not substitute zero, its Scan estimate, or before/after differences. Display success, partial failure, and failure without inventing per-file results.
- [x] Closing during an active empty operation waits for that shell call to return, starts no further drive operations, suppresses automatic rescan, and exits through the existing cooperative shutdown flow.
- [x] Use a narrow fake shell-call seam to verify drive-scope reuse, ordered results, unavailable counts, drive failures, and closing while a call is pending. These tests cannot touch a real Recycle Bin.
- [ ] Validate the actual shell operations and mixed folder/Recycle Bin workflow only in an isolated Windows environment with a disposable bin. Verify the remaining content and UI results, including the post-Clean Scan.

## Comments

- 2026-09-29: `src/core/recycle_bin.rs` wraps `GetLogicalDrives`/`GetDriveTypeW`, `SHQueryRecycleBinW`, and `SHEmptyRecycleBinW` (no confirmation, progress UI, or sound) behind a `Shell` trait. Each call names one drive root, never an empty root that would cover every drive. A Scan reports the drives it covered with its result; the controller captures them at Clean request time, including through the Scan-to-Clean handoff, and Clean empties only those drives. Clean queries each drive first and empties only nonempty bins, because the shell can report an error when asked to empty an empty bin. Stop is checked before every shell call, so closing waits for a pending call and starts no other drive. `CleanResult::deleted_bytes` is now `Option<u64>`; the Recycle Bin reports `None` and no skip counts. Its row shows *Emptied*, *Partly emptied*, or *Clean failed*, and the result line reads `Recycle Bin emptied (size unavailable)` on success; failures count as incomplete Targets. The per-Target worker loop moved from the UI into `core::scan_targets`/`core::clean_targets`.
- Fake-shell tests cover per-drive totals and an empty bin as complete, partial and failed drive queries, drive-scope reuse when a drive appears after the Scan, partial and failed empties, stopping while an empty call is pending, and ordered events for a mixed folder and Recycle Bin Scan and Clean. A controller test covers drive capture through handoff and rescan, and a format test covers the mixed result line. `cargo test --lib` passes 65 tests.
- A non-elevated debug build Scanned Kevin's real machine: the Recycle Bin row appeared under Windows, ticked by default, with a 1.4 GB shell estimate, and the new `recycle-bin: true` default was saved alongside the existing choices. Clean was not run. Kevin's Selection file was restored afterward.
- 2026-09-30, PR review: a Scan now records each drive's volume GUID path with its letter. Clean checks the letter again after querying the bin and skips it as `drive changed since Scan` if another volume is mounted there, so it cannot empty a bin the Scan never covered. A remount in the moment between that check and the empty call is still undetectable, because the shell names a bin only by drive letter. The real volume lookup succeeded for every fixed drive in a Scan on Kevin's machine.
- 2026-09-30: Kevin confirmed that emptying the Recycle Bin works in the app.
- Not run against a real bin: an already-empty bin (which would show whether the query before each empty is needed), a mixed Clean with folder Targets, a drive that fails, and closing during the empty call.
