# GPUI validation record

This record covers issue 01: the portable GPUI app with fixture data. Rerun these checks whenever the GPUI revision, the Rust toolchain, or the manifest changes.

## Build identity

| Item | Value |
|---|---|
| GPUI | `gpui` and `gpui_windows` from `zed-industries/zed` at `933d8d93819c749a607e561883855a9b95c79cea` (2026-09-25), default features off |
| Toolchain | Rust 1.98.1, `x86_64-pc-windows-msvc`, pinned in `rust-toolchain.toml` |
| Lockfile | `Cargo.lock` pins every dependency. Commit it with the source. |
| C runtime | Linked statically (`.cargo/config.toml`) |
| Manifest | `resources/app.manifest`: `requireAdministrator` in release builds and `asInvoker` in debug builds, PerMonitorV2 DPI awareness, Common Controls 6, `supportedOS` for Windows 10 and later (the GUID Windows 11 also uses) |
| Release size | About 7 MB, one file |

GPUI's own `windows-manifest` feature is off, because it embeds a second manifest that cannot require elevation. `resources/app.manifest` carries GPUI's DPI and Common Controls settings instead.

## Building

- Release builds compile HLSL shaders with `fxc.exe` from the Windows SDK. GPUI looks in `PATH`, then `C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\`. Set `GPUI_FXC_PATH` to use another location.
- `cargo build --release` produces `target\release\cc-cleaner.exe`.
- `cargo run` starts a debug build as the current user. Debug builds do not ask for elevation, so system Targets report access errors unless the terminal runs as administrator.
- Release builds require elevation, so start them through the shell, for example `Start-Process targetelease\cc-cleaner.exe`. `cargo run --release` fails with OS error 740 unless the terminal runs as administrator.
- Tests live in the library crate, whose test binary carries no manifest. `cargo test` runs unelevated.

## Graphics requirements

GPUI renders with Direct3D 11 through DirectComposition (`crates/gpui_windows/src/directx_devices.rs`, `directx_renderer.rs`):

- DXGI 1.6 (`IDXGIFactory6`), which all Windows 11 versions include.
- Direct3D feature level 11.1, 11.0, or 10.1 on the first adapter that supports it. The Microsoft Basic Render Driver also qualifies.
- If no adapter works, the app shows an error dialog, because it has no console.

## Tested environments

| Environment | Result |
|---|---|
| Windows 11 Pro 10.0.26200, x64, AMD Radeon integrated graphics and NVIDIA RTX 4090, 100% display scale, UAC set to elevate administrators without prompting | Pass |

## Results on Windows 11 (2026-09-26)

Automated checks used UI Automation from an elevated PowerShell session, plus `PrintWindow` screenshots.

- **Launch and packaging.** The release executable, copied alone to an empty temporary folder, starts elevated and renders. It imports only Windows system DLLs, with no VC++ runtime and no Winsock, WinHTTP, WinINet, or DNS imports.
- **Network.** `scripts/trace-network.ps1` records the Winsock AFD, Kernel-Network, DNS-Client, and WinHTTP ETW providers. The trace covers launch, checkbox changes, the Scan-to-Clean handoff, Clean, the automatic rescan, and window close. The app created no sockets and owned no network events.
  - ETW stamps kernel receive work with the thread that it interrupted. Thus other processes' traffic can show the app's process ID. The script attributes Kernel-Network events by their `PID` field and ignores AFD receive indications.
  - A `curl` run in the same trace produced socket create and connect events. This shows that the trace captures traffic.
  - The trace did not cover the error dialogs for startup failure and panics.
- **Layout.** The window is 520 × 640 DIP and fixed-size. The total and Rescan stay at the top, the checklist scrolls, and Clean and the result line stay at the bottom. A divider appears under the header when the list is scrolled. The scrollbar thumb follows the scroll position and can be dragged.
- **Fixture flows.** Progressive Scan, selected-first order, hidden absent Targets, and partial and failed rows all render. Ticking a partial Target disables Clean. When you select Clean during a slow unticked Scan, the app stops that Scan and then starts the Clean. Queued, cleaning, and final rows render, and the result line persists through the automatic rescan.
- **Theme.** Light, dark, and high contrast apply while the app is open, including the title bar. The accent color follows the system accent. High contrast uses the system palette.
- **Text size.** 150% and 225% apply while the app is open, and nothing clips. The window grows with the text size, stays centered, and stays inside the work area.
  - This is a design decision: the PRD asks for a window of approximately 520 × 640 DIP. At 225%, a fixed window leaves space for only a few rows, so the window grows with the text instead.
- **Keyboard.** Tab and Shift+Tab cycle through Rescan, the visible rows, and Clean (19 stops), skipping disabled controls. Space toggles checkboxes. Space and Enter press buttons. Enter does not toggle checkboxes, as in Windows. A focused row scrolls into view. The focus rectangle shows during keyboard use. A button that disables itself keeps focus, so Tab continues from it.
- **Accessibility tree.** UI Automation exposes these elements:
  - A Button named Rescan, and a Button named `Clean approximately …`.
  - A List named Targets, with heading Text for each Category.
  - A CheckBox for each Target, with its toggle state. Its description gives the row status.
  - A StatusBar that shows the result line. It is a polite live region, so AccessKit raises `LiveRegionChanged` when its text changes.
  - Disabled controls report `IsEnabled = false`.
- **Accessibility actions.** The UI Automation Toggle and Invoke patterns work, also on a row that is scrolled out of view.

## Verification scripts

Both scripts need an elevated PowerShell session, because the app runs elevated.

- `scripts/dump-uia.ps1` prints the window's UI Automation tree. Use `-Toggle <name>` or `-Invoke <name>` to act on a control first.
- `scripts/trace-network.ps1 -Exe <path> -Dir <output folder>` runs the network trace described above.
