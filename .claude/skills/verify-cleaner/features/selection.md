# Selection

The Selection is the set of ticked Targets. Each tick or untick is saved immediately, survives restarts, and feeds the header's selected estimate and the Clean button label. If the saved file can't be read, every Target starts unticked and the error is shown.

## Sub-features

- `selection-toggle` ticks or unticks a Target row and updates the header's selected count and estimate.
- `selection-persist` writes the change to `selection.json` at once and reloads it on the next launch.
- `selection-locked` disables the rows while a Clean runs.
- `selection-error` starts with all rows unticked and shows the read error in the status line when `selection.json` is malformed.

## How to get to it (user POV)

- Click a Target row or its checkbox, or press Space on a focused row.
- Relaunch the app to see the saved Selection.

## Driving it with verify.ps1

Preconditions:

- The app is launched and `$V doctor` shows `scan: idle`.
- Note the starting `toggle=` state of the row you will flip (from `$V tree`).

- **Toggle.** Run `$V toggle "pip cache"`. It prints the new state, e.g. `pip cache toggle=On`, after the Selection save finishes.
- **Header.** Run `$V tree -Out selection-toggled.txt`. The header's `<n> Targets selected` changes by one. After a Scan, the selected estimate and the `Clean approximately <size>` label include that row's size.
- **Stored.** Read `$APPDATA/cc-cleaner-at-home/selection.json`. The Target's id (`pip-cache`) has the new value.
- **Persist.** `stop` restores the pre-launch file, so carry the changed one across the restart. Copy `selection.json` to the run folder as `selection-toggled.json`, run `$V stop`, copy it back over `selection.json`, then `$V launch` and `$V tree`. The row keeps its new `toggle=` state. The next `$V stop` restores the user's original Selection.
- **Error.** With the app stopped, write `{not json` to `selection.json`, then `$V launch`. The launch wait succeeds once Scan is enabled. `$V tree` shows every row `toggle=Off` and a `StatusBar` naming the Selection error. `$V stop` restores the real file.

## Gotchas

- Rows are `[disabled]` during a Clean. `toggle` refuses disabled controls.
- The user's real Selection is rarely the default (README's "Ticked by default" column). Read it, don't assume it.
