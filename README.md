# cc-cleaner-at-home

A portable Windows 11 app for reviewing and permanently deleting caches and discarded files. See `CONTEXT.md` for the domain terms and `.scratch/cleaner-v1/` for the spec and issues.

## Build and run

- `cargo run` starts a debug build as the current user. Debug builds do not ask for elevation, so system Targets report access errors unless the terminal runs as administrator.
- `cargo build --release` builds `target\release\cc-cleaner.exe`, the single file that ships. It requires elevation, so start it with `Start-Process` or from File Explorer. `cargo run --release` fails with OS error 740 unless the terminal runs as administrator.
- Release builds compile GPUI's shaders with `fxc.exe` from the Windows SDK. Set `GPUI_FXC_PATH` if GPUI cannot find it.
- `cargo test` runs the unit tests. CI also runs `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`.

## Platform notes

- GPUI renders with Direct3D 11 at feature level 10.1 or higher. The Microsoft Basic Render Driver qualifies when there is no GPU.
- The C runtime is linked statically, so the executable needs no VC++ redistributable.
- `build.rs` embeds `resources/app.manifest`. GPUI's own manifest feature is off, because it cannot require elevation.

## Checks after a GPUI or toolchain update

Run both scripts from an elevated PowerShell session, because the app runs elevated.

- `scripts/dump-uia.ps1` prints the window's UI Automation tree: control names, roles, and checked and disabled states. Use `-Toggle <name>` or `-Invoke <name>` to act on a control first.
- `scripts/trace-network.ps1 -Exe <path> -Dir <output folder>` traces network activity during launch, checkbox changes, Clean, and close. The app must own 0 network events. A curl run at the end shows that the trace captures traffic.
