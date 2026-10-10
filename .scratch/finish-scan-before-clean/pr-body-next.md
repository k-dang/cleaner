## Summary
- Disable Clean until the full Scan completes, and ignore Clean requests made while scanning.
- Remove pending-Clean handoff coordination and its UI messaging.
- Update workflow documentation and add controller and UI coverage for the new sequence.

<!-- before-and-after:start -->
| Preview (Scan in progress — selected Targets are ready, Clean remains disabled) |
|:---:|
| ![Preview](.scratch/finish-scan-before-clean/captures/during-scan.jpg) |

| Preview (Before Scan) |
|:---:|
| ![Preview](.scratch/finish-scan-before-clean/captures/before-scan.jpg) |

<!-- before-and-after:end -->

## Testing
- Windows workspace formatting, Clippy with warnings denied, and all 75 workspace tests passed.
- Controller and rendered fixture UI checks cover Scan completion gating and keyboard focus behavior.
- Live Windows check confirmed Clean stays disabled during Scan; completion behavior was verified by controller and fixture UI tests.
