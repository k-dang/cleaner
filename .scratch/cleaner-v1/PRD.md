Status: ready-for-agent

# Spec: cc-cleaner-at-home v1

Domain terms (Target, Minimum age, Category, Default selection, Selection, Scan, Clean) are defined in the root `CONTEXT.md`. This spec uses them with those meanings.

## Problem Statement

Kevin's Windows disk fills up with junk: temp files, browser caches, shader caches, crash dumps, and developer package caches. CCleaner can remove this junk, but it has become bloated. It has telemetry, upsells, and bundled software, and it has risky features such as registry cleaning and driver updates. By default, it also deletes user data such as cookies and history, which logs him out of every site. He wants to see what takes up space and delete it safely with a few clicks, and he wants no network traffic and no surprises.

## Solution

A small, portable Windows desktop app with one screen. When it opens, it Scans a fixed list of Targets and shows a checklist grouped by Category, with the size of each Target. Kevin reviews the checklist and clicks one button to Clean the Selection. The app deletes only files that nobody needs to keep: caches that their owners rebuild, and files that were already discarded. It never deletes cookies, history, or other user data. It makes no network calls, stores only the Selection, and has no settings screen.

## User Stories

1. As Kevin, I want the app to start a Scan when it opens, so that I see how much space I can free without an extra click.
2. As Kevin, I want each Target row to show its size as soon as its Scan finishes, so that I do not wait for the slowest Target to see results.
3. As Kevin, I want to see the total size that I can free at the top of the window, so that I know at a glance if a Clean is worth it.
4. As Kevin, I want Targets grouped under the Categories Windows, Browsers, Apps, and Developer, so that I can find a Target quickly.
5. As Kevin, I want a tick box on each Target row, so that I control what a Clean deletes.
6. As Kevin, I want the Clean button to show the size of the current Selection, so that I know what will be deleted before I click.
7. As Kevin, I want the Clean button to be disabled until the Scan finishes, so that I do not Clean based on incomplete numbers.
8. As Kevin, I want no confirmation dialog after I click Clean, so that the review checklist is the only confirmation and I am not trained to click through prompts.
9. As Kevin, I want each selected row to show a spinner during a Clean and then a check mark with the freed size, so that I can see the progress.
10. As Kevin, I want one result line after a Clean ("Freed 14.2 GB · 214 files skipped (in use)"), so that I know what happened.
11. As Kevin, I want the app to Scan again after a Clean, so that the sizes show the new state of the disk.
12. As Kevin, I want a small rescan icon, so that I can refresh the sizes after I use other apps.
13. As Kevin, I want my Selection to be remembered between runs, so that I do not untick the same Targets each time.
14. As Kevin, I want the Selection to be saved at the moment I change a tick box, so that nothing is lost if the app closes.
15. As Kevin, on the first run, I want the Selection to come from each Target's Default selection, so that the app is useful without setup.
16. As Kevin, I want Targets that are costly to rebuild (developer package caches, NVIDIA shader cache) to be unticked by default, so that I do not have to download packages again or get game stutter by accident.
17. As Kevin, I want Targets that are not on my machine (for example, Firefox if it is not installed) to be hidden, so that the checklist shows only what is relevant.
18. As Kevin, I want files that are in use to be skipped and counted, so that a Clean never forces a delete or breaks an app that is running.
19. As Kevin, I want the app to never close my other apps, so that I never lose work in a browser or editor.
20. As Kevin, I want temp files younger than 24 hours to be kept, so that installers and apps that are running do not lose their working files.
21. As Kevin, I want the size shown for a temp Target to count only files that a Clean would delete, so that the numbers match the result.
22. As Kevin, I want cleaned files to be deleted permanently, so that the space is actually free and not moved to the Recycle Bin.
23. As Kevin, I want the Recycle Bin to be a Target, so that I can empty it together with the other junk.
24. As Kevin, I want browser caches cleaned, but never cookies, history, sessions, passwords, or form data, so that I stay logged in and keep my tabs.
25. As Kevin, I want Chrome, Edge, and Brave caches cleaned for every browser profile, so that the space in the other profiles is freed too.
26. As Kevin, I want the Windows Update download cache and the Delivery Optimization cache as Targets, so that I can free space that Windows leaves behind.
27. As Kevin, I want crash dumps and Windows error reports as Targets, so that old failure reports stop using space.
28. As Kevin, I want the thumbnail and icon cache as a Target, so that I can free the space that Explorer uses.
29. As Kevin, I want developer caches (npm, pnpm, Bun, Yarn, pip, uv, Cargo, NuGet, Go) as Targets, so that I can free many GB when I need space.
30. As Kevin, I want a Clean to never follow a symlink or junction, so that a link inside a cache can never cause a delete in another place, such as my user folder.
31. As Kevin, I want a Clean to delete only the contents of a Target folder and keep the folder, so that the owning app still finds its folder.
32. As Kevin, I want all Target paths to be fixed in the app, so that no input or file can make the app delete something else.
33. As Kevin, I want to close the window during a Clean, so that I can stop it. The result is still safe.
34. As Kevin, I want the app to make no network calls, so that nothing about my machine leaves it.
35. As Kevin, I want one UAC prompt at launch, so that all Targets, including the Windows system Targets, are always available.
36. As Kevin, I want one portable exe, so that I can run the app without an installer.
37. As Kevin, I want the window to follow the Windows light or dark theme, so that it looks like part of Windows.
38. As Kevin, I want the window to look native (Segoe UI Variable, Fluent-style spacing and controls), so that the app feels trustworthy.
39. As Kevin, I want one fixed-size window with no menu bar, settings screen, tray icon, or splash screen, so that nothing distracts from the task.
40. As Kevin, I want the checklist to scroll inside the window, so that the total and the Clean button are always visible.
41. As Kevin, I want a Target that fails to Scan (for example, access denied on the root folder) to show a clear state instead of a wrong size, so that I can trust the numbers.

## Implementation Decisions

### Platform and delivery

- Only Windows 10 and 11. To support another OS later, add Targets. The design does not change.
- Tauri 2 app. The backend is Rust. The frontend is plain TypeScript with Vite and no UI framework, because the app has one screen.
- Delivered as one portable exe. No installer, no code signing, no auto-update.
- The application manifest requires administrator (`requireAdministrator`). The app has one privilege state. Because Kevin is an admin user, the elevated process still sees his own user folders.
- No network access of any kind: no telemetry, no update checks, no downloaded Target definitions.

### Modules

1. **Core (targets, scan, clean)**: the deep module and the only test seam.
   - A fixed table of Targets. Each Target has an id, a display name, a Category, a Default selection, an optional Minimum age, and one or more folders. Each folder is a base root plus a fixed relative path. A folder can have an optional filename pattern (thumbnail cache) and an optional profile wildcard (browser profiles).
   - `Roots`: the base folders `LocalAppData`, `AppData`, `WinDir`, `ProgramData`, and `UserProfile`. The app fills them from the Windows known-folder APIs. Tests fill them with a temporary fixture folder.
   - `scan(targets, roots)` returns, for each Target, one of: not present (no folder exists, so the UI hides it), a size in bytes (only files a Clean would delete), or failed.
   - `clean(targets, roots)` returns, for each Target, freed bytes and a skipped file count.
   - Scan and Clean report each Target's result as soon as it is done, so the UI can update rows one at a time. Targets are processed in parallel.
2. **Recycle Bin**: a thin wrapper around `SHQueryRecycleBinW` (Scan) and `SHEmptyRecycleBin` (Clean, without the confirmation UI or sound). The core does not delete files in `$Recycle.Bin`.
3. **Selection store**: reads and writes the ids of the ticked Targets as JSON in the app's `%APPDATA%` folder. If there is no file, the store uses each Target's Default selection. If a Target id is unknown to the stored Selection (a new Target in a later version), the store uses that Target's Default selection. It ignores stored ids that no longer exist.
4. **Tauri commands and events**: commands to start a Scan, start a Clean with the current Selection, and read or write the Selection. Events send per-Target Scan and Clean results to the frontend.
5. **Frontend**: one screen that renders the total, the Categories with Target rows, the Clean button, and the result line. It keeps no state other than what the backend reports.

### Safety rules

1. Never follow a symlink, junction, or any other reparse point. Delete the link itself, not its destination. Do not count the destination's size in a Scan.
2. Delete the contents of a Target folder. Never delete the Target folder itself.
3. Target paths are fixed in the Target table and resolved against `Roots`. No path comes from user input or from a file.
4. Empty the Recycle Bin only through the Windows API.

### Clean behavior

- Deletion is permanent.
- If a file is locked or access is denied, skip it and count it. Never close other apps. Never take ownership or change permissions to force a delete.
- A Target with a Minimum age deletes only files whose last-modified time is older than that age. Folders that become empty are removed, except the Target folder itself.
- The user cannot cancel a Clean. If the window closes during a Clean, the disk is left in a safe state.
- After a Clean, the app shows the freed total and the skipped count, then starts a new Scan.

### Targets (v1)

For browsers, the per-profile folders repeat for each profile folder (`Default`, `Profile 1`, and so on). Confirm each folder on a real machine before implementation.

| Category | Target | Folders | Minimum age | Default selection |
|---|---|---|---|---|
| Windows | User temp | `LocalAppData\Temp` | 24 hours | Ticked |
| Windows | Windows temp | `WinDir\Temp` | 24 hours | Ticked |
| Windows | Recycle Bin | Windows API | - | Ticked |
| Windows | Thumbnail and icon cache | `LocalAppData\Microsoft\Windows\Explorer` (`thumbcache_*.db`, `iconcache_*.db` only) | - | Ticked |
| Windows | Windows Update downloads | `WinDir\SoftwareDistribution\Download` | - | Ticked |
| Windows | Delivery Optimization cache | `WinDir\ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache` | - | Ticked |
| Windows | Crash dumps and error reports | `LocalAppData\CrashDumps`, `LocalAppData\Microsoft\Windows\WER`, `ProgramData\Microsoft\Windows\WER\ReportArchive`, `ProgramData\Microsoft\Windows\WER\ReportQueue` | - | Ticked |
| Windows | DirectX shader cache | `LocalAppData\D3DSCache` | - | Ticked |
| Browsers | Chrome cache | `LocalAppData\Google\Chrome\User Data\<profile>\{Cache, Code Cache, GPUCache}`, `User Data\{ShaderCache, GrShaderCache}` | - | Ticked |
| Browsers | Edge cache | Same layout under `LocalAppData\Microsoft\Edge\User Data` | - | Ticked |
| Browsers | Brave cache | Same layout under `LocalAppData\BraveSoftware\Brave-Browser\User Data` | - | Ticked |
| Browsers | Firefox cache | `LocalAppData\Mozilla\Firefox\Profiles\<profile>\cache2` | - | Ticked |
| Apps | Discord cache | `AppData\discord\{Cache, Code Cache, GPUCache}` | - | Ticked |
| Apps | VS Code cache | `AppData\Code\{Cache, CachedData, Code Cache, GPUCache}` | - | Ticked |
| Apps | NVIDIA shader cache | `LocalAppData\NVIDIA\{DXCache, GLCache}` | - | Unticked |
| Developer | npm cache | `LocalAppData\npm-cache` | - | Unticked |
| Developer | pnpm store | `LocalAppData\pnpm\store` | - | Unticked |
| Developer | Bun cache | `UserProfile\.bun\install\cache` | - | Unticked |
| Developer | Yarn cache | `LocalAppData\Yarn\Cache` | - | Unticked |
| Developer | pip cache | `LocalAppData\pip\cache` | - | Unticked |
| Developer | uv cache | `LocalAppData\uv\cache` | - | Unticked |
| Developer | Cargo registry | `UserProfile\.cargo\registry` | - | Unticked |
| Developer | NuGet packages | `UserProfile\.nuget\packages` | - | Unticked |
| Developer | Go build cache | `LocalAppData\go-build` | - | Unticked |

### UI

- One fixed-size window, about 520 × 640 px.
- It follows the Windows light or dark theme. It uses Segoe UI Variable, Fluent-style spacing, and Fluent-style controls.
- Top: the total size that can be freed, and the rescan icon.
- Middle: a list that scrolls, with Category headings and Target rows (tick box, name, size or state).
- Bottom: the Clean button with the Selection size, and the last result line.

## Testing Decisions

- A good test checks only external behavior at the seam: build a fixture folder tree, call `scan` or `clean` with `Roots` that point to it, and check the returned results and the files that remain on disk. Tests must not depend on the internal structure of the core.
- The core module is the only module with automated tests. The cases:
  - A Minimum age filter keeps young files and deletes old files. The Scan size counts only the old files.
  - A junction inside a Target: the Clean removes the junction, the destination and its files stay, and the Scan does not count the destination.
  - The Target folder still exists after a Clean.
  - A locked file (opened without share-delete during the test) is skipped and counted, and the rest of the Target is deleted.
  - A filename pattern (thumbnail cache) deletes only the files that match.
  - A profile wildcard (browser) cleans the cache folders of every profile and leaves other profile data, such as a cookies file, untouched.
  - A Target with no existing folder is reported as not present.
- No automated tests for the Recycle Bin wrapper, the Selection store, the Tauri commands, or the UI.
- One manual end-to-end run of the real app on Kevin's machine: accept the UAC prompt, watch the Scan, change the Selection, Clean, check the freed total and the skipped count, then restart the app and check that the Selection was saved.
- There are no existing tests in the repository to follow, because the repository is new.

## Out of Scope

- Registry cleaning, startup management, uninstalling apps, driver updates, and "health checks".
- Privacy cleaning: cookies, history, saved sessions, passwords, and form data.
- Custom Targets added by the user.
- A view of the individual files inside a Target, or excluding them.
- Scheduled or background cleaning, and a tray icon.
- A history of past Cleans.
- A confirmation dialog before a Clean, and a cancel button.
- Detecting or closing apps that are running.
- Platforms other than Windows 10 and 11.
- An installer, code signing, auto-update, and any network access.
- A mode that runs without administrator rights.

## Further Notes

- Sizes measured on Kevin's machine on 2026-09-25 show which Targets matter most: NVIDIA shader cache 12 GB, NuGet 9.8 GB, Recycle Bin 1.4 GB, Edge profile 891 MB (whole profile, not only cache), thumbnail cache 604 MB. User temp, Chrome, npm, pnpm, and Bun were too large to measure in 60 seconds.
- Possible later additions, only when a real need appears: more app Targets (for example Slack, Teams, Spotify, Steam, JetBrains, Docker), and a warning before a Clean when an app that owns a Target is running, if skipped totals become large.
- The repository is not a git repository yet.
