# 01 - Build and validate the portable GPUI app

**What to build:** A portable Windows app with the actual cleaner screen, using fixture data to demonstrate reviewing the Selection and seeing Scan and Clean progress. This establishes that GPUI meets the delivery and UI requirements before deletion is connected. Follow the feature PRD's Platform and delivery, UI, and Initial GPUI check sections.

**Blocked by:** None - can start immediately.

**Status:** done

- [x] Build a Rust/GPUI Windows x64 release executable with embedded assets and the required elevation manifest. Pin compatible GPUI dependencies and commit the dependency lockfile. Record the tested revision and graphics requirements.
- [x] Render the intended window with Category headings, a scrolling Target checklist, estimated totals, Selection size, Rescan, Clean, and the last result line. Keep the total and action areas visible while scrolling.
- [x] Use fixture data to exercise checkbox changes, progressive Scan results, queued and active Clean rows, completed results, and partial or failed results. Fixture actions cannot delete real content. Keep this screen as the application UI for the next ticket.
- [x] Match the PRD's Windows-style appearance, spacing, typography, and window size. Light, dark, and high-contrast changes apply while the app is open; text scaling does not clip controls or results.
- [x] Verify Tab and Shift+Tab navigation, Space toggling, button activation, and visible focus. The UI Automation tree exposes control names, roles, checked/disabled states, and result updates correctly.
- [x] Verify the copied single executable launches with elevation on Windows 11 without development tools or separately installed application runtimes. Verify graphics initialization on the test machine.
- [x] Inspect attempted network traffic during startup, fixture interactions, and error paths. The executable and its dependencies initiate none; testing with the network disconnected alone is insufficient evidence.
- [x] Record the tested environments and results. Resolve packaging, graphics, accessibility, and visual defects before treating this ticket as complete. Keep the implementation limited to the controls this app needs.

## Comments

- 2026-09-26: Implemented and verified on Windows 11 Pro 10.0.26200 (x64, AMD Radeon integrated and NVIDIA RTX 4090, 100% display scale) with GPUI at zed `933d8d9`. Checks used UI Automation and `PrintWindow` screenshots from an elevated session.
  - Packaging: the release exe, about 7 MB, runs when copied alone to an empty folder. It imports only Windows system DLLs, with no VC++ runtime and no network libraries.
  - Network: an ETW trace of launch, checkbox changes, the Scan-to-Clean handoff, Clean, the automatic rescan, and close found no network events owned by the app. A curl run in the same trace was captured. The error dialogs were not traced.
  - UI: the total and Clean stay fixed while the checklist scrolls. Fixture Scan, partial and failed rows, the handoff, Clean progress, and the persistent result line all render.
  - Theme and text size: light, dark, high contrast, and 150% and 225% text apply while the app is open, with no clipping. Decision: the window grows with the text size, up to the work area, instead of staying at 520 x 640.
  - Keyboard: Tab and Shift+Tab cycle through all controls. Space toggles checkboxes, Space and Enter press buttons, focused rows scroll into view, and the focus rectangle shows.
  - Accessibility: UI Automation exposes names, roles, checked and disabled states, and a polite live region for the result line. Toggle and Invoke work, also on rows scrolled out of view.
