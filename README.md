# cc-cleaner-at-home

A small, offline Windows 11 app for reviewing and permanently deleting caches and discarded files. It is a personal replacement for CCleaner: one screen, a fixed list of cleanup Targets, honest size estimates, and no network access.

**[Download for Windows 11 x64](https://github.com/k-dang/cleaner/releases/latest/download/cc-cleaner.exe)** · [All releases](https://github.com/k-dang/cleaner/releases)

The download link becomes available when the first stable release is published.

<p align="center">
  <img src="docs/images/review.png" alt="Cleaner window after a Scan, showing Targets grouped by Category with size estimates and the Clean button" width="420">
</p>

## How it works

1. **Scan.** Click **Scan**. The app Scans every Target and fills in each row as it finishes, starting with the ticked ones. The header shows the estimated size of the Selection.
2. **Review.** Tick or untick Targets. The app saves each change right away. Developer caches and the DirectX shader cache start unticked because rebuilding them costs time or downloads.
3. **Clean.** Click **Clean approximately …**. The checklist is the confirmation. Deletion is permanent, and the app reports what it deleted, what it skipped, and why. It then Scans again.

<p align="center">
  <img src="docs/images/scanning.png" alt="Cleaner window mid-Scan, with some rows still scanning and one Target reporting an incomplete result" width="420">
</p>

Rows update while the Scan runs, and you can change the Selection as results arrive. Clean stays disabled until the entire Scan finishes. A selected Target that could not be fully inspected shows the reason, such as *Access denied*, and blocks Clean until you untick it or rescan. Incomplete results for unticked Targets do not block Clean after Scan finishes.

See [`CONTEXT.md`](CONTEXT.md) for the domain terms (Target, Minimum age, Selection, Scan, Clean).

## Targets

| Category | Target | What gets deleted | Minimum age | Ticked by default |
|---|---|---|---|---|
| Windows | User temp | `%LOCALAPPDATA%\Temp` | 24 hours | Yes |
| Windows | Windows temp | `%WINDIR%\Temp` | 24 hours | Yes |
| Windows | Recycle Bin | Your Recycle Bin on each local fixed drive, emptied through Windows | – | Yes |
| Windows | Thumbnail cache | `thumbcache_*.db` in `%LOCALAPPDATA%\Microsoft\Windows\Explorer` | – | Yes |
| Windows | Crash dumps and error reports | `%LOCALAPPDATA%\CrashDumps`, per-user and machine WER report folders | – | Yes |
| Windows | DirectX shader cache | `%LOCALAPPDATA%\D3DSCache` | – | No |
| Browsers | Chrome cache | `Cache`, `Code Cache`, and `GPUCache` in each Chrome profile | – | Yes |
| Developer | pnpm store | `%LOCALAPPDATA%\pnpm\store` | – | No |
| Developer | pip cache | `%LOCALAPPDATA%\pip\cache` | – | No |
| Developer | Go build cache | `%LOCALAPPDATA%\go-build` | – | No |

The table is fixed in [`src/targets.rs`](src/targets.rs). Neither the saved Selection nor the UI can add paths.

### Why these Targets

A cache folder existing is not enough. Each Target is admitted on a primary reference or an owner check: [`scripts/owner-checks`](scripts/owner-checks) fills a scratch cache, deletes part of it the way a partial or interrupted Clean can, and runs the owner again.

| Target | Decision | Evidence |
|---|---|---|
| User temp, Windows temp | Admitted | Applications create working files there; the 24-hour Minimum age keeps recent ones. |
| Recycle Bin | Admitted | Queried and emptied through `SHQueryRecycleBinW` and `SHEmptyRecycleBinW`. |
| Thumbnail cache | Admitted | Windows Disk Cleanup registers a `Thumbnail Cache` handler. Explorer keeps some files open; those are skipped. |
| Crash dumps and error reports | Admitted | Disk Cleanup cleans `Windows Error Reporting Files`; `CrashDumps` is WER's default local dump folder. |
| DirectX shader cache | Admitted, unticked | Disk Cleanup registers a `D3D Shader Cache` handler. |
| Chrome cache | Admitted | Chrome loaded pages and rebuilt its caches after concurrent, partial, and full deletion, keeping `Preferences`. |
| pnpm store, pip cache, Go build cache | Admitted, unticked | Installs and builds produced identical results after partial and interrupted deletion. |
| npm cache | Excluded | After partial deletion, the next install failed with `ENOENT`. |
| Bun, Yarn, Cargo registry caches | Excluded | They trust partly deleted package folders and install or build with missing files. |
| Icon cache; Edge, Brave, Firefox, Discord, VS Code, NVIDIA caches | Excluded | No owner check or primary reference for deleting them yet. |

## Safety

- **Only listed contents.** The app deletes eligible contents of each Target folder and keeps the folder itself. It never touches cookies, history, saved sessions, passwords, or form data.
- **No link following.** It never follows symlinks, junctions, or other reparse points. It works through directory handles, so a folder renamed or swapped during a Clean cannot redirect deletion elsewhere.
- **Minimum age.** The temp Targets keep files modified within the 24 hours before the operation starts, and Clean rechecks the age. This lowers the risk of deleting working files but does not prove a file is unused.
- **Recycle Bin through Windows.** The app queries and empties the Recycle Bin only through the Windows shell, never by opening `$Recycle.Bin`. Clean empties only the drives the latest Scan covered. Windows reports no per-file results, so the result says the bin was emptied without a deleted size.
- **No forcing.** It never takes ownership, changes permissions, closes apps, or schedules deletion on reboot. Rejected deletions are skipped and reported by reason.
- **Offline.** The app makes no network calls and has no telemetry or updater.
- **Estimates stay estimates.** Hard links, compression, and running apps can make reclaimed disk space differ from the deleted file sizes.

### Validation status

Automated tests run real Scans and Cleans against temporary folders. A release is done when the checks pass and Kevin has run the build on his machine and confirmed that it works.

## Install

Download `cc-cleaner.exe` from the [latest release](https://github.com/k-dang/cleaner/releases/latest), copy it anywhere, and run it. There is no installer, and it needs no VC++ redistributable or other runtime. Release builds ask for administrator rights once at launch so they can reach `%WINDIR%\Temp` and the machine-wide error reports. You can also build it yourself (below).

Releases are currently unsigned. Windows may show an unknown publisher or SmartScreen warning, or block the app under Smart App Control.

To update, close the app and replace the executable with the new download.

The Selection is stored in `%APPDATA%\cc-cleaner-at-home\selection.json`. If that file is malformed or unreadable, the app starts with every Target unticked and shows the error. Nothing else is persisted.

Each release includes `cc-cleaner.exe.sha256`. To check a download, run `Get-FileHash .\cc-cleaner.exe -Algorithm SHA256` in PowerShell and compare the hash with that file.

## Build and run

Building the desktop app requires Windows 11 x64 and the Rust toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml). The portable core can be built and tested on Windows, macOS, and Linux.

- `cargo run` starts a debug build as the current user. It scans the real Target folders. Debug builds do not ask for elevation, so Windows temp and crash reports can report access errors unless the terminal runs as administrator.
- `cargo build --release` builds `target\release\cc-cleaner.exe`, the single file that ships. It requires elevation, so start it with `Start-Process` or from File Explorer. `cargo run --release` fails with OS error 740 unless the terminal runs as administrator.
- Release builds compile GPUI's shaders with `fxc.exe` from the Windows SDK. Set `GPUI_FXC_PATH` if GPUI cannot find it.
- `cargo test --workspace --locked` runs the portable workflow and Selection checks plus the Windows cleanup, Target table, Selection store, startup, checklist, and formatting checks.
- `cargo test -p cleaner-core --locked` runs only the portable core on any of the three platforms, without building GPUI or Windows dependencies. CI runs this and core Clippy on Windows, macOS, and Linux.
- Windows CI also runs `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings`.

### Architecture and portability

The workspace has two packages. [`cleaner-core`](crates/cleaner-core/src/lib.rs) owns Target metadata, Selection parsing and serialization, Scan/Clean coordination, and results. The root `cc-cleaner-at-home` package owns the Windows desktop app: GPUI, startup, system appearance, durable Selection storage, the fixed Windows Target catalog, and [`cleanup`](src/cleanup.rs).

The controller receives a catalog from the native app. Each Scan result carries a native Scan snapshot, which the controller retains without interpreting it and passes back with that Target's Clean request. Windows folder Targets retain their handle-based cleanup procedure; the Recycle Bin snapshot retains only the volumes that Scan covered. Starting a new Scan discards the previous snapshots, and late reports cannot replace current ones.

The desktop app and releases remain Windows-only. A second native app must supply its own verified Targets, equivalent deletion protections, storage, and UI. See [ADR 0001](docs/adr/0001-portable-core-and-windows-app.md) for the separation decision.

## Release

1. Update the version in `Cargo.toml` and run `cargo check` to update `Cargo.lock`. Commit both files along with any release changes and merge them into `main`.
2. From the merged commit, create and push the matching tag. For version `0.1.0`:

   ```powershell
   git tag -a v0.1.0 -m "Release v0.1.0"
   git push origin v0.1.0
   ```

3. The [Release workflow](.github/workflows/release.yml) validates the version, runs the checks, and builds with locked dependencies. It creates a **draft** release with the executable, checksum, and generated change notes. Prerelease versions such as `0.2.0-rc.1` are marked as prereleases.
4. Download the draft's executable, verify its checksum, and test that exact build on your Windows 11 machine.
5. Review the notes and publish the draft. For a stable release, select **Set as latest release** to update the README download link.

The workflow uses GitHub's built-in token and needs no additional secrets.

## Platform notes

- The UI is built with [GPUI](https://gpui.rs/), pinned to one Zed revision in `Cargo.toml`. It renders with Direct3D 11 at feature level 10.1 or higher. The Microsoft Basic Render Driver qualifies when there is no GPU.
- The window follows the Windows light, dark, and high-contrast themes and the Windows text size, including changes while it is open.
- The C runtime is linked statically, so the executable needs no VC++ redistributable.
- `build.rs` embeds `resources/app.manifest` and `resources/app.ico`. GPUI's own manifest feature is off, because it cannot require elevation. The icon uses resource ID 1, which GPUI loads for the window. To change it, replace `resources/app.ico` with a transparent, multi-size Windows icon and rebuild.

## Checks after a GPUI or toolchain update

Run both scripts from an elevated PowerShell session, because the app runs elevated.

- `scripts/dump-uia.ps1` prints the window's UI Automation tree: control names, roles, and checked and disabled states. Use `-Toggle <name>` or `-Invoke <name>` to act on a control first.
- `scripts/trace-network.ps1 -Exe <path> -Dir <output folder>` traces launch, the displayed User temp and Windows temp checkbox changes (restoring the original Selection), a manual Scan, and close. The app must own 0 network events, and a successful curl positive control must own network events. The report excludes Clean and the automatic post-Clean rescan, because the script exercises only non-destructive controls.

`scripts/owner-checks` holds the owner-behavior checks used to admit or exclude Targets. They damage scratch caches and a scratch browser profile, never the real ones, and need network access.
