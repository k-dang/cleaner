# 03 - Add the remaining folder-based Targets

**What to build:** Extend the existing review-and-Clean workflow to the remaining Windows, browser, app, and developer folder Targets in the feature PRD. Each admitted Target works through the same checklist, saved Selection, Scan, Clean, and result flow. Validate each Target before admitting it; remove any that cannot meet the existing cleanup contract.

**Blocked by:** 02 - Complete the temp cleanup workflow.

**Status:** ready-for-agent

- [ ] Account for every proposed folder Target beyond the temp Targets: thumbnail/icon cache, crash dumps/error reports, DirectX shader cache, Chrome, Edge, Brave, Firefox, Discord, VS Code, NVIDIA, npm, pnpm, Bun, Yarn, pip, Cargo, and Go.
- [ ] For each Target, confirm actual standard local locations and owner versions in a disposable Windows environment. Record a primary reference or reproducible owner-behavior check, plus evidence for concurrent use, rejected deletions, interruption, and subsequent owner recovery. Folder existence alone does not establish safety.
- [ ] Add only validated Targets. Remove unsupported Targets from product definitions and UI rather than leaving disabled placeholders. Record the disposition and evidence for every proposed Target so coverage is reviewable. Keep cleanup within the existing procedure; do not add owner-command execution or service management to make an incompatible Target fit.
- [ ] Scan and Clean each admitted Target through the existing workflow with correct Category, Default selection, persistent choices, estimates, and error states. Developer Targets and the named DirectX and NVIDIA shader Targets are unticked by default. Hide absent Targets and empty Categories without losing saved choices.
- [ ] Thumbnail/icon cleanup applies its filename filter only to immediate files. Multi-folder Targets distinguish missing folders from inaccessible folders, and all Target roots remain intact after Clean.
- [ ] Browser profile expansion covers the PRD's supported standard profiles and fixed cache children. Reject redirected profiles and ancestors. Fixture tests prove cookies, history, saved sessions, passwords, and form-data sentinels remain untouched across multiple profiles.
- [ ] Validate included GPU cache rebuild effects and developer cache behavior under concurrent package activity and partial cleanup. A Target that requires another cleanup protocol is excluded rather than relying on its unticked default for safety.
- [ ] Add focused behavioral tests for filename filters, profile expansion, multi-folder outcomes, and any new eligibility behavior. Reuse existing core safety and lifecycle tests rather than duplicating them for every table row.
- [ ] Demonstrate a mixed-Category Scan and Clean in the packaged app on disposable data, including an absent app, a partial failure, an unticked developer cache, and owner rebuild after interruption. UI results and remaining files agree with the documented cleanup behavior.
