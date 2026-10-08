# cc-cleaner-at-home

A personal, offline Windows desktop app for reviewing and permanently deleting selected caches and discarded files. Cookies, history, saved sessions, passwords, and form data are excluded.

## Language

**Target**:
A built-in cleanup choice with a defined set of eligible caches or discarded files and a verified cleanup procedure. Being a cache does not by itself establish that arbitrary deletion is safe.
_Avoid_: Rule, cleaner, location

**Minimum age**:
The time since a file's last modification that must pass before it is eligible for a Clean. It reduces the chance of deleting active working files but does not prove that a file is unused.
_Avoid_: Threshold, retention

**Category**:
A heading that groups related Targets in the checklist, such as Windows, Browsers, or Developer.
_Avoid_: Group, section, tab

**Default selection**:
Whether a Target is ticked when no choice has been saved for it. Targets that are costly to rebuild, such as developer and shader caches, are unticked by default.
_Avoid_: Recommended, preset

**Selection**:
The set of Targets the user has ticked. Explicit ticks and unticks are remembered between runs, and each Clean uses the Selection captured when it starts.
_Avoid_: Profile, settings, config

**Scan**:
Estimating the size of eligible content in each Target without changing it. The estimate can differ from a later Clean result or the disk space actually reclaimed.
_Avoid_: Analyze, check

**Scan snapshot**:
The information a Target's Scan records to constrain a later Clean when its cleanup procedure requires it. A snapshot belongs to that Target and the latest Scan; it does not freeze the files or make the estimate exact.
_Avoid_: Token, plan, cached paths

**Clean**:
Attempting permanent deletion of eligible content in the captured Selection using each Target's verified procedure. It reports completed work and failures without forcing deletion or guaranteeing that running apps are unaffected.
_Avoid_: Wipe, purge, run
