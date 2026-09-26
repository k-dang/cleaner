# cc-cleaner-at-home

A personal, offline Windows desktop app that frees disk space by deleting junk files. It is a simple, trustworthy replacement for CCleaner: it only removes files nobody needs to keep, and never touches user data such as cookies or history.

## Language

**Target**:
A known location whose contents nobody needs to keep, so deleting them permanently is safe. Either the owner rebuilds them on demand (caches), or they were already discarded (Recycle Bin, crash dumps, old logs).
_Avoid_: Rule, cleaner, location

**Minimum age**:
The age a file in a Target must reach before a Clean deletes it. Only temp Targets have one (24 hours), because apps write to them while they run.
_Avoid_: Threshold, retention

**Category**:
A heading that groups related Targets in the checklist, such as Windows, Browsers, or Developer.
_Avoid_: Group, section, tab

**Default selection**:
Whether a Target is ticked the first time the app runs. Targets that are costly to rebuild (such as developer package caches) are unticked by default.
_Avoid_: Recommended, preset

**Selection**:
The set of Targets the user has ticked. It is remembered between runs.
_Avoid_: Profile, settings, config

**Scan**:
Measuring how much space each Target currently uses, without changing anything. It starts when the app opens.
_Avoid_: Analyze, check

**Clean**:
Permanently deleting the contents of the Targets in the Selection. Files that are in use are skipped and counted, never forced.
_Avoid_: Wipe, purge, run
