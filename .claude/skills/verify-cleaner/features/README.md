# Cleaner verification map

This directory is the maintained source for verifying the user-facing behavior of Cleaner. Read this index before driving the app, then use the matching feature file as the recipe. `$V` is the helper defined in [`../SKILL.md`](../SKILL.md).

## Baseline preconditions

- No Cleaner is running except one started by this run (`tasklist | grep -i cc-cleaner` is empty before `launch`).
- `cargo build --locked` succeeded and `$V doctor` shows no `STALE` build.
- `$V launch` printed `READY`. The app starts unscanned, with the user's own saved Selection. Do not assume the default ticks.
- The run folder from `launch` is where every artifact goes.

## Driving conventions

- Use accessible names exactly as the UIA tree shows them. Target names come from `src/targets.rs`.
- Act through `$V toggle` and `$V invoke` only. Do not edit `selection.json` by hand except where a recipe names it as a precondition.
- Wait with `$V wait-scan` or `$V wait-name`, not fixed sleeps.
- Read the Selection before acting, and leave it as you found it. `$V stop` restores the file, but restoring through the UI keeps later steps honest.
- Never invoke `Clean*` without the user's explicit go-ahead in this conversation.

## Proof and skip reporting

- UI proof is a `tree -Out` before and after the action plus a `shot` of the end state.
- Selection proof adds the content of `%APPDATA%\cc-cleaner-at-home\selection.json` after the change.
- Clean proof adds a directory listing of the cleaned folder before and after.
- Record the feature ID with every artifact (`scan-match.txt`, not `out.txt`).
- Report each unverified sub-feature with its reason. "Clean not run: no consent" is a valid result. Calling Clean verified because tests pass is not.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph describing the user-visible behavior, then exactly four H2 sections in this order: `Sub-features`, `How to get to it (user POV)`, `Driving it with verify.ps1` (starts with `Preconditions:`), and `Gotchas`. Keep implementation details out; name user paths, stable handles, required state, commands, and observable proof.

## Features

- [Scan](./scan.md) covers the manual Scan, live per-row progress, header totals, and incomplete results.
- [Selection](./selection.md) covers ticking Targets, persistence across restarts, and an unreadable Selection file.
- [Clean](./clean.md) covers Clean gating, the destructive Clean itself, the result summary, and the automatic rescan. It requires user consent.
