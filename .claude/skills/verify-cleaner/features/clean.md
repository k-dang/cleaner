# Clean

Clean permanently deletes the eligible content of the ticked Targets, reports what it deleted and skipped per row and in a summary, and then rescans. It is enabled only after a complete Scan, when no ticked Target is incomplete.

## Sub-features

- `clean-gating` keeps `Clean` disabled before and during a Scan, and while a ticked Target is incomplete.
- `clean-run` locks the rows, shows `Cleaning… x of y Targets finished`, and marks each queued row `Queued`, `Cleaning…`, then `Deleted <size>` (or `Emptied` for the Recycle Bin).
- `clean-summary` shows the result in the status line, e.g. `Deleted <size> of files`, `<n> files skipped (<reason>)`.
- `clean-rescan` starts a Scan automatically when the Clean finishes.

## How to get to it (user POV)

- Click `Clean approximately <size>` (or `Clean` when the estimate is zero) at the bottom right after a Scan.

## Driving it with verify.ps1

Preconditions:

- **The user has explicitly approved running Clean in this conversation, naming the Targets.** Without that, verify only `clean-gating` and report the rest as not run.
- A complete Scan (`$V wait-scan` after `$V invoke Scan`).
- Exactly the approved Targets are ticked. Untick all others with `$V toggle`. The lowest-risk choice is a single small Developer cache such as `Go build cache`, which its owner rebuilds.

- **Gating, before Scan.** Right after `launch`, `$V tree` shows `Clean` as `[disabled]`.
- **Gating, during Scan.** After `$V invoke Scan`, `$V tree` shows `Clean approximately <size>` still `[disabled]` until `wait-scan` returns.
- **Gating, incomplete.** Tick an incomplete row (e.g. `Crash dumps and error reports` when unelevated). Its desc ends `Untick or rescan to Clean` and `Clean` is `[disabled]`. Untick it again.
- **Before, on disk.** List the approved folder, e.g. `ls "$LOCALAPPDATA/go-build" | head` and `du -sh "$LOCALAPPDATA/go-build"`.
- **Run.** Run `$V tree -Out clean-before.txt`, then `$V invoke "Clean approximately*"`, then `$V tree -Out clean-during.txt`. The rows are `[disabled]` and the StatusBar reads `Cleaning… x of y Targets finished`.
- **Result.** Run `$V wait-scan` (the automatic rescan finishes too), then `$V tree -Out clean-after.txt` and `$V shot clean-after.png`. The StatusBar holds the summary, and the cleaned row shows its new, smaller size.
- **After, on disk.** Repeat the listing. The folder still exists and its eligible contents are gone or reduced, matching the summary.

## Gotchas

- There is no dry run, test mode, or redirect. Clean always hits the real folders under the user's profile.
- For temp Targets, files modified in the last 24 hours are kept, so "after" is rarely empty.
- Files held open by running apps (Explorer's thumbnail cache, Chrome while open) are skipped and reported. That is correct behavior.
- The Recycle Bin reports `Emptied` with no size. Windows provides no per-file results.
- After the rescan, the per-row `Deleted <size>` is replaced by fresh Scan sizes. Capture `clean-during.txt` or the summary if you need the per-row result.
