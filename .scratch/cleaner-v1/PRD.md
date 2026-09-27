Status: draft - Target validation required before implementation readiness

# Spec: cc-cleaner-at-home v1

Domain terms (Target, Minimum age, Category, Default selection, Selection, Scan, Clean) are defined in the root `CONTEXT.md`. This spec uses them with those meanings.

## Problem statement

Kevin's Windows disk fills up with temp files, browser caches, shader caches, crash dumps, and developer package caches. He wants a small replacement for CCleaner that lets him review and delete selected content with a few clicks. It must preserve browser user data, make no network calls, and show honest estimates and results.

## Solution

A portable Windows desktop app with one screen. It Scans a fixed list of Targets on launch and shows a checklist grouped by Category. Kevin reviews the estimates, changes the Selection, and clicks Clean. The checklist is the confirmation; deletion is permanent.

Each Target has a verified cleanup procedure. Cache files can be needed by running apps, and old temp files can still be working files. A Minimum age and skipping rejected deletions reduce risk but do not establish that content is unused. The app never deletes cookies, history, saved sessions, passwords, or form data.

## User stories

1. As Kevin, I want a Scan on launch, with each Target's result shown as it finishes, so that I can review results without waiting for every Target.
2. As Kevin, I want the total eligible size and the Selection size shown as estimates, so that I can judge whether a Clean is worth it.
3. As Kevin, I want Targets grouped under Windows, Browsers, Apps, and Developer, with a tick box on each row, so that I control the Selection.
4. As Kevin, I want to Clean once all selected, present Targets have complete Scan results, so that a slow unticked Target does not block me.
5. As Kevin, I want the checklist to be the only confirmation before Clean, so that I do not have to dismiss another prompt.
6. As Kevin, I want each selected row to show queued, cleaning, and final results, so that I can distinguish completed work from skipped or failed work.
7. As Kevin, I want one result line after Clean and a new Scan, so that I can see what happened and review current estimates.
8. As Kevin, I want a rescan icon, so that I can refresh results after using other apps.
9. As Kevin, I want both ticks and unticks saved when I change them, with a visible error if saving fails, so that exclusions are not silently lost.
10. As Kevin, I want new Targets to use their Default selection while existing choices stay unchanged, so that an update preserves my Selection.
11. As Kevin, I want developer caches and the DirectX and NVIDIA shader caches unticked by default, so that I choose when to incur downloads or rebuild costs.
12. As Kevin, I want absent Targets hidden and failed or incomplete Scans shown clearly, so that missing content is not confused with an access error.
13. As Kevin, I want rejected deletions skipped and reported by reason, so that Clean does not force access or claim that every failure means a file is in use.
14. As Kevin, I want other apps left running, so that Clean does not close my work.
15. As Kevin, I want temp files modified within the last 24 hours kept, so that recent working files are excluded from routine cleanup.
16. As Kevin, I want permanent deletion and a Recycle Bin Target, so that cleanup does not just move files into the Recycle Bin.
17. As Kevin, I want supported browser caches cleaned across their standard profiles while user data stays untouched.
18. As Kevin, I want thumbnail and icon caches, crash dumps, shader caches, and supported developer caches available as Targets, so that I can review the main sources of disk use.
19. As Kevin, I want cleanup confined to the listed Target contents even if a path is redirected or changes during the operation, so that it cannot delete content elsewhere.
20. As Kevin, I want to close the window to stop further cleanup, so that the app can exit after any operation already in progress finishes.
21. As Kevin, I want one portable executable with no network calls or downloaded runtime, so that I can use it offline without an installer.
22. As Kevin, I want one elevation request at launch for system Targets, with access errors still reported honestly.
23. As Kevin, I want a Windows-style window that follows the Windows theme, with a scrolling checklist and fixed total and action areas, so that the controls remain visible.

## Implementation decisions

### Platform and delivery

- Windows 11, x64, for Kevin's local administrator account. The filesystem and privilege behavior is Windows-specific.
- Rust with [GPUI](https://gpui.rs/) for the UI. GPUI provides Windows windowing and GPU rendering without an embedded browser runtime. Use the `windows` crate for filesystem, elevation, and shell operations. Keep the application in one executable with embedded UI assets.
- Pin GPUI and its companion crates to a tested, compatible release or exact Git revision and commit `Cargo.lock`. GPUI is pre-1.0 and can introduce breaking changes; dependency updates require rerunning the UI and packaging checks. [GPUI documentation](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md).
- Validate the selected GPUI revision's graphics requirements on Windows 11. [Windows rendering implementation](https://github.com/zed-industries/zed/blob/main/crates/gpui_windows/src/window.rs).
- Statically link application runtime dependencies where required for the portable build; depend only on supported Windows system libraries at run time.
- The manifest uses `requireAdministrator`. Support consent elevation of Kevin's own account. Launch under another account is unsupported. Elevation does not guarantee access to every file and does not justify bypassing permissions.
- The executable and its dependencies must initiate no network traffic, including telemetry, crash uploads, update checks, and runtime downloads. This requirement does not claim to control unrelated Windows services. Verify the shipped build's traffic, including startup and failures.
- Resolve only local filesystem locations. Reject UNC paths and mapped network drives before enumeration so redirected user folders cannot cause network access.
- Persist only the Selection. Do not persist Scan results, cleanup history, or diagnostic logs.

### Modules

1. **Core (Targets, Scan, Clean)** owns Target eligibility, path confinement, traversal, deletion, and per-Target results. Its small interface accepts Target IDs, `Roots`, the operation time, a stop signal, and a result callback. Production callers cannot supply deletion paths. Tests supply fixture roots and a fixed time through the same interface.
   - A fixed Target table contains each ID, display name, Category, Default selection, optional Minimum age, and folders or the Recycle Bin operation. A folder uses a base root and a fixed relative path, with a limited filename filter or profile expansion where needed.
   - `Roots` contains `LocalAppData`, `AppData`, `WinDir`, `ProgramData`, and `UserProfile`, resolved through the appropriate Windows folder APIs. Validate the resolved locations before traversing them.
   - Scan and Clean share eligibility rules. Clean checks those rules again against the current files; a previous Scan never authorizes deletion by itself.
   - Process Targets sequentially on a worker thread. Report each result as it finishes and check the stop signal between filesystem steps. Add bounded concurrency only if measured performance establishes a need.
   - Keep the Recycle Bin wrapper inside the core. The core exposes its different result capabilities honestly. Do not add a plugin system or generic cleanup strategy framework.
2. **Selection store** loads and durably saves choices by Target ID. Its interface accepts a complete set of choices and reports persistence failures.
3. **Controller** owns the Selection, Scan results, current operation, and last Clean result. It enforces operation exclusion and readiness independently of UI controls. It publishes ordered updates to the UI thread and acknowledges worker completion before starting another operation.
4. **GPUI UI** renders the controller's state and forwards user actions. It owns presentation details such as focus, scrolling, and theme changes, but does not implement deletion or eligibility rules. Deliver worker updates through GPUI's UI-thread update mechanism. Keep filesystem work off the UI thread and keep the core and Selection store independent of GPUI types so their tests need no window or GPU.

### Selection persistence

- Store a JSON object mapping every known Target ID to a boolean, including `false`, in the app's local `%APPDATA%` directory. For example: `{ "user-temp": true, "directx-shader-cache": false }`.
- A missing file means first run and uses Default selections. In a valid file, missing keys use Defaults, explicit booleans win, and unknown IDs are ignored. Preserve choices for temporarily absent Targets.
- Save the complete set of choices on first run or when new IDs receive defaults, before enabling Clean. If this initial save fails, show the error and keep Clean disabled until an explicit checkbox change saves successfully.
- Validate the complete file. An unreadable or malformed file is an error, not a first run. Show the error and start with every Target unticked. The user must make explicit choices before Clean; do not silently restore default ticks.
- Serialize writes. Write a complete replacement to a temporary file in the same directory, flush it, and atomically replace the saved file. Interrupted writes must leave the previous complete file or the new complete file, not truncated JSON. Ignore leftover temporary files when loading.
- Start a save on each checkbox change. Disable Clean while saving. A failed save restores the last successfully saved choices, or all unticked if none are available, and shows an error. A subsequent explicit change retries saving. After a load error, a successful explicit save establishes the new choices.
- Complete a pending save before a normal window close. Do not claim that an uncompleted write survives process termination or power loss.

### Safety rules

1. Delete only eligible contents of verified Target folders. Preserve each Target folder. Filename-filtered Targets inspect only the specified directory, not its descendants.
2. Never follow a symlink, junction, or other reparse point during Scan or Clean. If a root, intermediate directory, profile directory, or Target folder is redirected, skip that folder and report it. Do not delete a redirected Target root. An eligible symlink or junction found inside an ordinary Target can be removed as a link only; skip other reparse-point types whose removal behavior has not been verified. Never include destination bytes in an estimate.
3. Confinement must survive concurrent rename or replacement of directories. Use verified directory handles and Windows operations that preserve that confinement through deletion. A path-prefix check, canonicalization, or checking attributes and then deleting by an independently resolved path is insufficient. If safe traversal cannot be established, skip the affected content.
4. Restrict runtime requests to built-in Target IDs. Neither saved choices nor user input can add paths. Profile expansion matches only documented immediate profile directories and fixed cache children; it never expands to an entire profile.
5. Never take ownership, change permissions or read-only attributes, close other apps, stop services, or schedule deletion on reboot to force cleanup. Sharing violations and access-denied errors are distinct outcomes. Other I/O failures remain visible.
6. A successful delete does not prove that a file was unused. Windows may accept deletion while handles permit delete sharing. The 24-hour temp rule is a heuristic, not a guarantee about active installers or apps. Target admission must account for this. [Windows deletion behavior](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-deletefilew).
7. A temp file is eligible only when its last-modified time is strictly earlier than the operation's start time minus 24 hours. Keep files on the cutoff, with future timestamps, or with unreadable timestamps. Report unreadable metadata as incomplete coverage. Remove empty descendant directories only after eligible content is processed.
8. Empty the Recycle Bin only through the Windows shell API. Do not traverse or delete `$Recycle.Bin` directly.

### Scan and Clean results

Scan returns one of these states for each Target:

| State | Meaning | UI and Clean readiness |
|---|---|---|
| Complete | All applicable folders were inspected. Includes the sum of eligible logical file sizes. | Show an estimate. The selected Target is ready. |
| Not present | All configured folders are confirmed absent. | Hide the row and retain its saved choice. Do not include it in Clean. |
| Partial | Some content was inspected, but some could not be inspected safely or completely. | Label the known size incomplete and show the reason. Untick or rescan before Clean. |
| Failed | No reliable estimate is available because access or inspection failed. | Show the reason, not zero. Untick or rescan before Clean. |
| Stopped | The operation stopped before this Target finished. | Show no final estimate. Require another Scan before selecting it for Clean. |

A missing folder within a multi-folder Target is normal. An inaccessible folder is not absence. The Recycle Bin uses the shell's size estimate; an empty bin remains a visible, complete Target.

Clean returns complete, partial, failed, or stopped status, known logical bytes accepted for deletion when available, known file-skip counts by reason, and any coverage error. Complete means the procedure finished without rejected deletions or coverage errors; use partial when some work completed but errors remain, and failed when an error prevented any work. A Target can have deleted content and still be partial or stopped. An inaccessible subtree does not provide a known skipped-file count. Report the incomplete Target without inventing a count for unseen files. A file already removed by another process contributes no deleted bytes.

Sizes are estimates of content, not a promise of physical disk space. Hard links, compression, delayed deletion, and concurrent app activity can make reclaimed space differ. Count ordinary files from metadata associated with the successful deletion operation; do not infer deleted bytes from the pre-Clean estimate or a before/after Scan difference.

For the Recycle Bin, query with `SHQueryRecycleBinW` and empty with `SHEmptyRecycleBinW`, using the same drive scope. v1 covers the current account's bins on mounted local fixed drives enumerated at Scan time; Clean uses that captured drive list. Suppress confirmation, progress UI, and sound. Check each HRESULT. The empty operation returns no per-file results, so deleted bytes and skip counts are unavailable for this Target, even on success. Aggregate drive failures as partial or failed. [Query contract](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shqueryrecyclebinw), [empty contract](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shemptyrecyclebinw).

Examples of truthful result text:

- `Deleted 1.2 GB of files · 4 files skipped (sharing violation)`
- `Deleted 1.2 GB of files · Recycle Bin emptied (size unavailable)`
- `Deleted 640 MB of files · 2 Targets incomplete (access denied)`

Explain that deleted file sizes may differ from disk space reclaimed. Never display `Freed` as an exact measurement, convert unavailable counts to zero, or label all failures `in use`.

### Operation lifecycle

- Permit one app instance per user and one active Scan or Clean. The controller rejects duplicate operation requests. No two workers may traverse or delete concurrently.
- On launch, load the Selection and Scan selected Targets first, then unticked Targets. Rows update as each Target finishes. The top total states when scanning is still in progress and excludes unknown or incomplete sizes from its complete subtotal.
- Enable Clean when at least one selected, present Target has a complete result, every selected Target is complete or confirmed absent, and the Selection is saved. Absent Targets do not block it. Selected partial or failed Targets must be unticked or successfully rescanned.
- If Clean is clicked while only unticked Targets are still scanning, capture the ready Selection and request that Scan stop. Wait for its worker to acknowledge completion, then start Clean. Do not wait for an entire slow Target to finish; stop between filesystem steps. No deletion overlaps Scan.
- Capture the ready Target IDs and Recycle Bin drive scope at Clean request time. Disable Selection changes, Rescan, and duplicate Clean requests through handoff and Clean. Controller validation enforces these rules even if a command bypasses disabled controls.
- Rescan is available only when idle. It replaces previous estimates. Results from a completed or stopped operation cannot overwrite results from the next operation.
- On normal Clean completion, preserve its result line and start a fresh Scan. The new Scan replaces row results as it progresses. Do not automatically retry failed deletions.
- Closing the window requests cooperative stop and suppresses the automatic rescan. Stop issuing new deletions after the current filesystem step completes. A Recycle Bin API call already in progress cannot be cancelled by this wrapper; wait for it to return and do not start another drive. Keep a responsive `Stopping...` window until the worker and pending Selection save finish, then exit.
- There is no forced worker termination, rollback, or background cleanup after normal exit. Completed deletions stay permanent. Abrupt process termination can leave partial cleanup; each admitted Target must tolerate that interruption without requiring a later repair step.

### Targets (v1)

These are the proposed v1 locations, not evidence that they are safe to ship. Before implementation readiness, validate each retained Target on a disposable Windows installation: confirm its actual paths and owner/version, record a primary reference or reproducible owner-behavior check, and establish that the defined cleanup tolerates concurrent use, rejected deletes, and interruption. A folder's existence is insufficient. Remove any Target whose cleanup requires a different protocol that v1 does not implement. Do not add guessed paths or ship an unvalidated Target disabled by default.

Only standard local locations are supported. Custom cache directories and browser profiles outside these locations are not discovered through configuration files. Expand Chromium profiles only as immediate `Default` or `Profile <number>` directories, and Firefox profiles only as immediate directories below the listed `Profiles` folder. Apply confinement checks before entering each profile.

| Category | Target | Folders | Minimum age | Default selection |
|---|---|---|---|---|
| Windows | User temp | `LocalAppData\Temp` | 24 hours | Ticked |
| Windows | Windows temp | `WinDir\Temp` | 24 hours | Ticked |
| Windows | Recycle Bin | Windows shell API, local fixed drives | - | Ticked |
| Windows | Thumbnail and icon cache | `LocalAppData\Microsoft\Windows\Explorer`, immediate `thumbcache_*.db` and `iconcache_*.db` only | - | Ticked |
| Windows | Crash dumps and error reports | `LocalAppData\CrashDumps`, `LocalAppData\Microsoft\Windows\WER`, `ProgramData\Microsoft\Windows\WER\ReportArchive`, `ProgramData\Microsoft\Windows\WER\ReportQueue` | - | Ticked |
| Windows | DirectX shader cache | `LocalAppData\D3DSCache` | - | Unticked |
| Browsers | Chrome cache | `LocalAppData\Google\Chrome\User Data\<profile>\{Cache, Code Cache, GPUCache}` | - | Ticked |
| Browsers | Edge cache | Same profile cache layout under `LocalAppData\Microsoft\Edge\User Data` | - | Ticked |
| Browsers | Brave cache | Same profile cache layout under `LocalAppData\BraveSoftware\Brave-Browser\User Data` | - | Ticked |
| Browsers | Firefox cache | `LocalAppData\Mozilla\Firefox\Profiles\<profile>\cache2` | - | Ticked |
| Apps | Discord cache | `AppData\discord\{Cache, Code Cache, GPUCache}` | - | Ticked |
| Apps | VS Code cache | `AppData\Code\{Cache, CachedData, Code Cache, GPUCache}` | - | Ticked |
| Apps | NVIDIA shader cache | `LocalAppData\NVIDIA\{DXCache, GLCache}` | - | Unticked |
| Developer | npm cache | `LocalAppData\npm-cache` | - | Unticked |
| Developer | pnpm store | `LocalAppData\pnpm\store` | - | Unticked |
| Developer | Bun cache | `UserProfile\.bun\install\cache` | - | Unticked |
| Developer | Yarn cache | `LocalAppData\Yarn\Cache` | - | Unticked |
| Developer | pip cache | `LocalAppData\pip\cache` | - | Unticked |
| Developer | Cargo registry | `UserProfile\.cargo\registry` | - | Unticked |
| Developer | Go build cache | `LocalAppData\go-build` | - | Unticked |

Both named shader Targets are unticked. Browser and app cache validation must account for the rebuild effects of the included `GPUCache` directories.

### UI

- Build the screen with GPUI's layout and rendering primitives and Windows-style controls. The app owns the controls' appearance, interaction, and accessibility behavior. Implement only the controls needed by this screen.
- One fixed-size window, approximately 520 by 640 device-independent pixels. Scale controls and text with Windows DPI and text settings; keep them usable without clipping.
- Follow Windows light, dark, and high-contrast preferences, including changes while the window is open. Use Segoe UI Variable. Implement Tab and Shift+Tab navigation, Space to toggle focused checkboxes, standard button activation, and visible focus. Expose control names, roles, checked/disabled states, and result updates to Windows accessibility tools.
- Top: estimated total eligible size and Rescan. Clearly distinguish a running or incomplete Scan from a complete total.
- Middle: a scrolling checklist with Category headings and Target rows. Hide empty Categories. Show size estimates, reasons for incomplete results, and operation progress in the affected rows.
- Bottom: `Clean approximately 1.2 GB`, or `Clean` when the complete estimate is zero, and the persistent last result line. Explain the difference between deleted file sizes and reclaimed disk space in concise supporting text.
- Clean progress distinguishes queued and active Targets. Use a check mark only for a complete result; partial and failed results show their reason. Keep the final result line visible during the subsequent Scan.
- Use brief inline text for saving failures and shutdown waits. No menu bar, settings screen, tray icon, splash screen, or extra confirmation dialog.

## Verification

### Initial GPUI check

- Before connecting deletion, build a release executable with the real window layout, a scrolling fixture checklist, selection controls, and simulated progress and results. Verify keyboard navigation, the UI Automation tree, theme changes, and text scaling. Use this screen as the app UI after validation; do not maintain a separate demo or UI framework.
- On Windows 11, verify launch with the elevation manifest, graphics initialization on the test machine, and execution from a copied single executable without development tools or separately installed application runtimes. Embed all required assets and inspect attempted network traffic. Record the tested GPUI revision and graphics requirements; resolve failures before enabling cleanup.

### Focused automated checks

- Test core behavior through fixture roots and a fixed operation time. Check returned results and remaining files, not internal traversal structure.
- Cover Minimum age below, at, and above the cutoff; unreadable metadata; preservation of Target roots; shallow filename filters; and multiple browser profiles with user-data sentinel files left untouched.
- Cover confirmed absence, root access denial, and an inaccessible subtree. Verify complete, partial, and failed results without treating unknown sizes or counts as zero.
- Exercise a locked file opened without delete sharing, a file opened with delete sharing, and a file removed concurrently. Verify actual outcomes without claiming the file was unused or counting another process's deletion.
- Cover links inside a Target, redirected roots and intermediate directories, redirected profiles, and concurrent directory replacement. Use an outside sentinel tree and assert that neither Scan nor Clean visits or modifies it. A deterministic filesystem race test may use a narrow internal synchronization hook; a static junction test alone is insufficient.
- Verify cooperative stop leaves completed deletions intact, issues no new deletions after stop acknowledgment, and preserves content outside the captured Selection. Interrupt the process in a fixture test to verify tolerated partial cleanup.
- Test Selection first run, explicit unticks across restart, new IDs, obsolete IDs, absent Targets, malformed files, denied writes, ordered rapid changes, and interruption during replacement. Check that saving errors do not silently enable default choices.
- Test controller readiness with a slow unticked Scan, Selection capture during handoff, duplicate requests, selected partial results, ordered completion updates, close during Scan/Clean, and suppression of the post-Clean Scan on close. Use controlled completions instead of timing sleeps.
- Test Recycle Bin result mapping and drive-scope reuse with a narrow fake shell-call seam, including partial drive failure and unavailable counts. Test the real shell operation only in an isolated Windows environment.

### End-to-end acceptance

For issue 02, Kevin waived the isolated packaged Clean test and concurrent owner-behavior check as prerequisites for enabling Clean on 2026-09-27. These checks remain unperformed; the issue records the exception.

- Run the real packaged executable first in a disposable Windows account or VM with seeded Target content, protected sentinel files, and a disposable Recycle Bin. Do not use Kevin's live caches as the first destructive test.
- Exercise launch/elevation, progressive Scan, Selection persistence, a slow unticked Target, Clean, partial failures, automatic rescan, close during cleanup, and restart. Verify the files that remain as well as the UI results.
- Verify retained Targets' owning apps still work and rebuild their caches after cleanup and interruption. Record the owner versions and evidence with the corresponding Target definitions. Any failing Target must be removed or have its cleanup corrected before it ships.
- Check the portable executable offline on Windows 11 without development tools or separately installed application runtimes. Inspect process network activity during launch, Scan, Clean, and error paths; an unplugged-network test alone does not prove absence of attempted traffic.
- Inspect light, dark, high-contrast, keyboard focus, scrolling, and text scaling, including changes while the window is open. Verify that the UI Automation tree exposes checkbox states, disabled controls, and result updates correctly. Verify the total, action controls, error text, and last result remain readable.
- Only after isolated acceptance passes, repeat the intended review-and-Clean flow on Kevin's machine with an explicitly reviewed Selection.

## Out of scope

- Registry cleaning, startup management, uninstalling apps, driver updates, and health checks.
- Privacy cleaning of cookies, history, saved sessions, passwords, and form data.
- User-defined Targets, per-file inspection, exclusions, and discovery of custom cache locations.
- Scheduled or background cleanup, a tray icon, and cleanup history.
- Detecting or closing running apps, managing services, or forcing access.
- A confirmation dialog before Clean and a separate cancel button.
- Other operating systems, other account/elevation modes, and nonlocal filesystems.
- Installers, code signing, auto-update, and network access.

## Existing observations

Measurements recorded on Kevin's machine on 2026-09-25 include NVIDIA shader cache at 12 GB, Recycle Bin at 1.4 GB, and thumbnail cache at 604 MB. An Edge profile measured 891 MB, but this included user data and is not a cache estimate. User temp, Chrome, npm, pnpm, and Bun did not finish measurement within 60 seconds. These observations motivate progressive results and allowing selected Targets to proceed independently of unfinished unticked Targets; they do not validate deletion safety.
