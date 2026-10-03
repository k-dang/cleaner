# 05 - Verify and deliver v1

**What to build:** The portable v1 executable. Fix defects found while running it, and confirm that the app works on Kevin's machine.

**Blocked by:** 03 - Add the remaining folder-based Targets; 04 - Add Recycle Bin cleanup.

**Status:** ready-for-review - Kevin's check on his machine is outstanding

- [x] Build the release executable from pinned dependencies with embedded assets and the elevation manifest. It runs when copied alone to Windows 11 without development tools or separately installed runtimes.
- [x] Fix defects found while running the app.
- [x] `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, and `cargo test --locked` pass.
- [ ] Kevin runs the release build on his machine, reviews the Selection, Cleans, and confirms that the app works.

## Comments

- 2026-10-03: Running the release build found and fixed three defects:
  - After a live Windows text-size change, the layout used the previous window size until the next change. `fit_window` now resizes in a later task, as GPUI's own `Window::resize` does, because GPUI drops the `WM_SIZE` that arrives while the window is being updated.
  - A file deleted while another process held it with delete sharing stays listed until that handle closes. Opening it fails with `STATUS_DELETE_PENDING`, which Win32 reports as access denied, so the next Scan marked the Target incomplete and blocked Clean. A pending deletion now counts as gone. A core test covers it.
  - Selection errors showed std's `(os error 5)` suffix in plain text. They now show Windows' message with the error color and icon used by rows, and every progress text uses `…`.
- The same run checked: the Scan-to-Clean handoff from a slow unticked Scan, duplicate requests, explicit unticks across restart, a selected partial Scan, Selection save and load errors, closing during a Scan, a folder deletion, and a Recycle Bin empty, owner recovery after a full Clean of every Target, light, dark, high-contrast, text-size, and keyboard changes, the UI Automation tree, and software rendering without a GPU. A network trace attributed no connections, DNS, or HTTP activity to the app.
