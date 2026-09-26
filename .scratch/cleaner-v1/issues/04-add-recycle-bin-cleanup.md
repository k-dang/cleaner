# 04 - Add Recycle Bin cleanup

**What to build:** A Recycle Bin Target that Kevin can select and empty in the same workflow as folder Targets. Use Windows shell operations and show their actual reporting limits. Follow the feature PRD's drive-scope, result, and shutdown contracts.

**Blocked by:** 02 - Complete the temp cleanup workflow.

**Status:** ready-for-agent

- [ ] Show the Recycle Bin under Windows with its Default selection and a shell-provided Scan estimate. An empty bin remains a visible, complete Target, and its saved choice survives restart.
- [ ] Enumerate the current account's mounted local fixed drives at Scan time. Query and Clean the same captured drive scope; do not silently broaden cleanup to drives that appeared later.
- [ ] Query and empty through the Windows shell API, suppressing confirmation, progress UI, and sound. Never traverse the Recycle Bin filesystem directly. Check each operation result and represent partial drive failures accurately.
- [ ] Integrate the Target with Selection capture, readiness, operation exclusion, row progress, the final result line, and automatic rescan. A mixed Clean can report ordinary deleted file sizes alongside the Recycle Bin outcome.
- [ ] Deleted bytes and per-file skip counts for the Recycle Bin remain unavailable, including on success. Do not substitute zero, its Scan estimate, or before/after differences. Display success, partial failure, and failure without inventing per-file results.
- [ ] Closing during an active empty operation waits for that shell call to return, starts no further drive operations, suppresses automatic rescan, and exits through the existing cooperative shutdown flow.
- [ ] Use a narrow fake shell-call seam to verify drive-scope reuse, ordered results, unavailable counts, drive failures, and closing while a call is pending. These tests cannot touch a real Recycle Bin.
- [ ] Validate the actual shell operations and mixed folder/Recycle Bin workflow only in an isolated Windows environment with a disposable bin. Verify the remaining content and UI results, including the post-Clean Scan.
