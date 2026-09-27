# cc-cleaner-at-home

A portable Windows 11 app for reviewing and permanently deleting caches and discarded files. See `CONTEXT.md` for the domain terms and `.scratch/cleaner-v1/` for the spec and issues.

## Current scope

The live worker supports User temp and Windows temp only. It scans files last modified more than 24 hours before the operation starts, then rechecks age during Clean. The age rule reduces risk but does not prove that a file is unused. Rejected deletions are skipped and reported. Other proposed Targets remain in the PRD and are not displayed by this build.

The app saves explicit ticks and unticks in `%APPDATA%\cc-cleaner-at-home\selection.json`. A malformed or unreadable Selection starts with both Targets unticked. The app shows save errors and rolls back a failed change.

The core and controller pass disposable-folder checks. The Clean button remains disabled because this release has not passed the PRD's first destructive packaged test in a disposable Windows account or VM. Enable Clean only after that test and concurrent owner-behavior validation pass.

## Build and run

- `cargo run` starts a debug build as the current user. It scans real temp folders. Debug builds do not ask for elevation, so Windows temp can report access errors unless the terminal runs as administrator.
- `cargo build --release` builds `target\release\cc-cleaner.exe`, the single file that ships. It requires elevation, so start it with `Start-Process` or from File Explorer. `cargo run --release` fails with OS error 740 unless the terminal runs as administrator.
- Release builds compile GPUI's shaders with `fxc.exe` from the Windows SDK. Set `GPUI_FXC_PATH` if GPUI cannot find it.
- `cargo test --lib` runs the core, Selection store, controller, and formatting checks. CI also runs `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`.

## Platform notes

- GPUI renders with Direct3D 11 at feature level 10.1 or higher. The Microsoft Basic Render Driver qualifies when there is no GPU.
- The C runtime is linked statically, so the executable needs no VC++ redistributable.
- `build.rs` embeds `resources/app.manifest`. GPUI's own manifest feature is off, because it cannot require elevation.

## Checks after a GPUI or toolchain update

Run both scripts from an elevated PowerShell session, because the app runs elevated.

- `scripts/dump-uia.ps1` prints the window's UI Automation tree: control names, roles, and checked and disabled states. Use `-Toggle <name>` or `-Invoke <name>` to act on a control first.
- `scripts/trace-network.ps1 -Exe <path> -Dir <output folder>` traces launch, the displayed User temp and Windows temp checkbox changes (restoring the original Selection), manual Rescan, and close. The app must own 0 network events, and a successful curl positive control must own network events. Missing required controls, failed interactions, timeouts, or trace failures abort verification. The report explicitly excludes Clean, the Scan-to-Clean handoff, and the automatic post-Clean rescan: Clean is currently disabled, and this script exercises only non-destructive controls.
