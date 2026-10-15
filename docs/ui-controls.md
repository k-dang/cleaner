# Reusable-control evaluation

Evaluated on 2026-10-14. Keep the current GPUI controls: the alternatives require a UI or dependency migration without a demonstrated reduction in complexity.

A replacement must preserve accessible row status, Space-to-toggle without Enter-to-toggle, disabled controls leaving the tab order, scrolling focused rows into view, system light/dark/high-contrast colors, and Windows text scaling.

The pinned GPUI revision has no ready-made checkbox, button, or scrollbar controls. Zed's separate `ui` crate supplies buttons and scrollbars but depends on its [theme, icon, and component infrastructure](https://github.com/zed-industries/zed/blob/933d8d93819c749a607e561883855a9b95c79cea/crates/ui/Cargo.toml).

[GPUI Kit](https://github.com/longbridge/gpui-kit/blob/main/docs/ARCHITECTURE.md) separates interaction from styling, but its [manifest](https://github.com/longbridge/gpui-kit/blob/main/Cargo.toml) uses `gpui-pre` snapshots. Cleaner uses git-pinned Zed `gpui`; their Rust types are not interchangeable. Adoption requires a dependency migration and validation of Windows startup, packaging, appearance, and accessibility.

Windows standard controls supply [accessibility support](https://learn.microsoft.com/en-us/windows/win32/winauto/accessibility-best-practices), but require new window, layout, and appearance integration to replace the GPUI screen. Neither migration was prototyped in this evaluation.
