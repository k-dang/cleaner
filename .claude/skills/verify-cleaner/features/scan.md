# Scan

Scan estimates the eligible size of every Target without changing anything. Rows fill in one at a time, ticked Targets first. The header tracks the selected estimate and progress, and Targets that could not be fully inspected show why.

## Sub-features

- `scan-start` shows the unscanned state at launch and starts a Scan from the `Scan` button.
- `scan-progress` updates rows (`Waiting` → `Scanning…` → size) and the header (`x of 10 scanned`) while Scan runs, with `Scan` disabled.
- `scan-complete` re-enables `Scan`, shows a total across Targets, and enables Clean when nothing selected is incomplete.
- `scan-incomplete` shows `<size> (incomplete). <reason>` on rows that could not be fully inspected, and excludes them from the header total.

## How to get to it (user POV)

- Click the `Scan` button at the top right of the window.
- The app also rescans automatically after a Clean (see [clean.md](./clean.md)).

## Driving it with verify.ps1

Preconditions:

- `$V doctor` shows `scan: idle`.
- Before the first Scan, the header contains `Scan to estimate the space you can reclaim` and every row is `desc="Not scanned"`.

- **Before.** Run `$V tree -Out scan-before.txt`. The header ends `Not scanned yet` and `Clean` is `[disabled]`.
- **Start.** Run `$V invoke Scan`, then immediately `$V tree -Out scan-during.txt`. `Scan` is `[disabled]`, the header reads `Scanning <Target>…` and `n of 10 scanned`, and unscanned rows read `Waiting`.
- **Finish.** Run `$V wait-scan`. It prints `idle` when `Scan` is enabled again.
- **After.** Run `$V tree -Out scan-after.txt` and `$V shot scan-after.png`. The header reads `<size> estimated across <n> Targets` and `Scan complete`, or `Excludes <n> Target(s) with incomplete results`. Every row shows a size or an incomplete reason, and `Clean approximately <size>` is enabled unless a ticked Target is incomplete.
- **Incomplete.** Unelevated debug builds typically show `Crash dumps and error reports` as `(incomplete). Access denied`. In the screenshot, the row has a warning icon and the reason text below the name.

## Gotchas

- Scan is read-only, but it walks real folders. A large pnpm store can take minutes. `wait-scan` defaults to a 600 s timeout.
- Size values depend on the machine's real caches, so assert the shape (a size or a reason per row), not specific numbers.
- If a ticked Target is incomplete, `Clean` stays disabled after the Scan. That is expected behavior, not a hang.
