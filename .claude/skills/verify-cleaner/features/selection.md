# Selection

The Selection is the set of ticked Targets. Each tick or untick takes effect immediately, queues a save for future runs, and feeds the header's selected estimate and the Clean button label. Save failures show an error while keeping the current ticks and allowing Clean once Scan is ready. If the saved file can't be read, every Target starts unticked and the error is shown.

## Sub-features

- `selection-toggle` ticks or unticks a Target row and updates the header's selected count and estimate.
- `selection-persist` saves changes in order in the background and reloads the latest saved choices on the next launch. Closing waits for queued writes.
- `selection-locked` disables the rows while a Clean runs.
- `selection-error` starts with all rows unticked and shows the read error in the status line when `selection.json` is malformed.

## How to get to it (user POV)

- Click a Target row or its checkbox, or press Space on a focused row.
- Relaunch the app to see the saved Selection.

## Driving it with verify.ps1

Preconditions:

- The app is launched and `$V doctor` shows `scan: idle`.
- Note the starting `toggle=` state of the row you will flip (from `$V tree`).

- **Toggle.** Run `$V toggle "pip cache"`. It prints the new state, e.g. `pip cache toggle=On`. Confirm the stored value separately; the checkbox updates before the background write finishes.
- **Header.** Run `$V tree -Out selection-toggled.txt`. The header's `<n> Targets selected` changes by one. After a Scan, the selected estimate and the `Clean approximately <size>` label include that row's size.
- **Stored.** Read `$APPDATA/cc-cleaner-at-home/selection.json`. The Target's id (`pip-cache`) has the new value.
- **Persist.** Keep the first run's original backup (or note that no file existed). Copy the saved `selection.json` to that run folder as `selection-toggled.json`, run `$V stop`, copy the changed file back over `selection.json`, then `$V launch` and `$V tree`. The row keeps its new `toggle=` state. After the final `$V stop`, restore the first run's original backup, or remove `selection.json` if none existed initially; the second run's backup contains the changed choices.
- **Error.** With the app stopped, write `{not json` to `selection.json`, then `$V launch`. The launch wait succeeds once Scan is enabled. `$V tree` shows every row `toggle=Off` and a `StatusBar` naming the Selection error. `$V stop` restores the real file.

## Gotchas

- Rows are `[disabled]` during a Clean. `toggle` refuses disabled controls.
- A save error affects whether choices are remembered next time; it does not revert ticks or block Clean. Failure handling is covered by the controller and UI tests without changing permissions on the user's real file.
- The user's real Selection is rarely the default (README's "Ticked by default" column). Read it, don't assume it.
