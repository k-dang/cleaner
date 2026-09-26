# 05 - Verify and deliver v1

**What to build:** The complete portable v1 executable, verified against the feature PRD in a disposable Windows 11 environment. Resolve defects found in the integrated workflow and provide the artifact with recorded acceptance evidence.

**Blocked by:** 03 - Add the remaining folder-based Targets; 04 - Add Recycle Bin cleanup.

**Status:** ready-for-agent

- [ ] Build the release executable from pinned dependencies with embedded assets and the elevation manifest. Verify it runs when copied alone to Windows 11 without development tools, an installer, or separately installed application runtimes. Record the build identity and tested graphics environments.
- [ ] Run the complete launch, progressive Scan, review, Clean, result, rescan, and restart workflow using seeded disposable data across the admitted Categories. Verify explicit unticks persist and actual remaining files agree with the displayed results.
- [ ] Exercise selected partial failures, slow unticked Scans, Scan-to-Clean handoff, duplicate requests, mixed folder/Recycle Bin outcomes, and closing during Scan, folder deletion, and a shell operation. Verify the app leaves no cleanup running after normal exit.
- [ ] Verify owner recovery after complete and interrupted cleanup for every shipped Target. Review the recorded evidence and dispositions from Target admission. Resolve any unsafe behavior before release; no unvalidated Target ships as a disabled placeholder.
- [ ] Inspect the packaged app's attempted network activity during launch, Scan, Clean, and error paths. Verify the executable and its dependencies initiate no traffic; record evidence beyond an offline launch test.
- [ ] Inspect light, dark, high-contrast, keyboard navigation, the UI Automation tree, scrolling, and text scaling, including live preference changes. Fix clipping, unreadable text, missing focus or accessible states, and inconsistent progress/error presentation.
- [ ] Run the project's build, formatting, lint, and focused automated checks. Resolve failures and flakiness, including issues discovered during acceptance, before delivery.
- [ ] Provide the portable artifact and concise acceptance evidence identifying tested environments, retained Targets, and any unmet requirement. An untested environment or unresolved requirement is not a pass. Any later Clean on Kevin's machine requires an explicitly reviewed Selection after isolated acceptance passes.
