---
name: verify-cleaner
description: Launch and drive the Cleaner Windows desktop app (cc-cleaner.exe, GPUI) through UI Automation to prove a change works the way a user sees it — Scan, Selection ticks, Clean gating, status text — and capture UIA trees and screenshots as evidence. Use after changing src/ui.rs, src/cleanup, src/selection.rs, targets, or the core controller, or whenever asked to verify, E2E test, or screenshot the app.
---

# Verify Cleaner

Cleaner is a single-window Windows app with no CLI, HTTP, or test-only hooks. You drive it the way Narrator sees it: UI Automation names exposed by GPUI's accessibility tree. All commands below run from the repo root in the Bash or PowerShell tool.

Read [`features/README.md`](features/README.md) before driving anything, then use the matching feature file as the recipe.

## Not on Windows?

The app builds and runs only on Windows 11, so nothing here can be driven on macOS or Linux. Verify what is portable there: `cargo test -p cleaner-core --locked` and `cargo clippy -p cleaner-core --all-targets --locked -- -D warnings`. Report the UI as not verified ("needs a Windows run") rather than substituting core tests for it.

## Hard limits

- **It acts on the real machine.** Target folders come from `SHGetKnownFolderPath`, so no env var or flag redirects them. Scan is read-only and safe. **Clean permanently deletes the user's real files.** Never invoke Clean without the user's explicit go-ahead in this conversation. Deletion logic is covered by `cargo test --workspace --locked` against temporary folders; prefer that for proving deletion behavior.
- **The Selection is real.** Ticks are saved to `%APPDATA%\cc-cleaner-at-home\selection.json`. The helper backs it up on `launch` and restores it on `stop`.
- **One instance per Windows account.** A global mutex makes a second launch show a "Cleaner is already running." message box and exit. No side-by-side runs. If a Cleaner you did not start is open, stop and ask the user to close it. Never drive or kill it.
- **Time-box.** Kevin finds long E2E sessions wasteful. State a short plan and estimate first, and check in after ~15 minutes.

## Helper

All driving goes through one script. Use Windows PowerShell (it has the UIA assemblies):

```sh
V="powershell -NoProfile -ExecutionPolicy Bypass -File .claude/skills/verify-cleaner/scripts/verify.ps1"
$V launch                      # start target\debug\cc-cleaner.exe, back up Selection, wait until idle
$V doctor                      # read-only health check of the recorded instance
$V tree [-Out tree.txt]        # print the UIA tree with row statuses; -Out saves it in the run folder
$V toggle "User temp"          # flip a Target checkbox and wait for the Selection save
$V invoke Scan                 # press a button by accessible name ('*' allowed: "Clean approximately*")
$V wait-scan                   # wait until Scan is enabled and no Selection load/save is pending
$V wait-name "Deleted*"        # wait until a control with this name ('*' allowed) exists
$V shot scan-done.png          # PNG of the window into the run folder (works even when covered)
$V stop                        # close our instance, restore Selection, keep evidence
```

The script records the instance in `target\verify\current.json` (pid, exe, run folder, Selection backup). Every command except `launch` acts only on that pid. `-Timeout <seconds>` (default 600) bounds every wait. A full Scan of a large pnpm store takes several minutes.

## Launch

1. Build: `cargo build --locked`. A cold build takes about 1–2 minutes; later builds are incremental.
2. Start: `$V launch`. Ready when it prints `READY pid=<n> run=<folder>`. That means the window exists, the Selection has loaded, and the Scan button is enabled.

The debug build runs unelevated as the current user. That is the right default. The release build (`target\release\cc-cleaner.exe`) requires elevation, so `launch -Exe` only works with it from an elevated shell. Unelevated, `Windows temp` and the machine-wide crash report folders may report `access denied`. That is expected and is itself a path worth seeing (see `features/scan.md`).

The app does not Scan on startup. The header reads `Scan to estimate the space you can reclaim` until you invoke Scan.

## Doctor

Run `$V doctor` first, and again whenever anything looks off. It prints the pid and exe, window title (`Cleaner`), whether the build is older than the newest file in `src/` or `crates/` (`STALE` → rebuild, `stop`, `launch`), whether Scan is idle, the header summary, and the Clean button's name. If it throws "not running", run `$V stop` to restore the Selection, then relaunch.

## Drive

Stable handles are accessible names from `src/ui.rs`:

| Control | UIA type | Name |
|---|---|---|
| Scan button | Button | `Scan` (disabled while scanning or cleaning) |
| Target row | CheckBox | Target name from `src/targets.rs`, e.g. `User temp`, `Chrome cache`; `toggle=On/Off`; `desc="..."` is the row status |
| Category heading | Text | `Windows`, `Browsers`, `Developer` |
| Header summary | Text | `Selected estimate: <size>. <overview>. <n> Targets selected · <scan note>` |
| Clean button | Button | `Clean` or `Clean approximately <size>`; disabled until the full Scan completes |
| Status line | StatusBar | Empty when idle; otherwise `Loading Selection…`, `Saving Selection…`, Selection errors, `Cleaning… x of y Targets finished`, or the Clean summary |

Use `$V tree` to read state. Each row's `desc="..."` is its status, read from UIA FullDescription: `Not scanned`, `Waiting`, `Scanning…`, a size such as `15 GB`, or `88 MB (incomplete). Access denied`. Prefer UIA patterns (Toggle, Invoke) over mouse clicks or keystrokes. Windows blocks a background script from taking focus, so `SendKeys`-style keyboard driving is unreliable; leave keyboard checks to the user. GPUI row clicks have an explicit accessibility handler, so `toggle` is the real user path for assistive technology.

## Evidence

Every run writes to `target\verify\<yyyyMMdd-HHmmss>\`. `stop` never deletes it, and it also saves `selection.after.json` next to `selection.before.json`. `cargo clean` does remove it, so copy anything worth keeping before that.

Proof standards:
- Capture the action and the resulting state: a `tree -Out` before and after, plus a `shot` of the end state.
- Go through the real user path: UIA toggle/invoke on visible controls. Do not edit `selection.json` by hand to fake a tick, except when setting up a precondition (e.g. a malformed file) that the feature file names.
- Verify side effects alongside the screen. For Selection, read `selection.json` after the toggle. For Clean (only with consent), check the target folder on disk before and after.
- Report what you did not verify and why (e.g. "Clean not exercised: no user consent"). Never report a skipped path as verified through another one.

## Cleanup

`$V stop` closes the window of the recorded pid (force-stops only that pid if it hangs for 30 s), restores the user's Selection file (or removes it if none existed before), and deletes `target\verify\current.json`. It never kills by process name. Run it after every attempt, including failed ones. Evidence stays in the run folder.
