# Reusable-control evaluation

Evaluated on 2026-10-14 while separating the single GPUI screen's responsibilities.

The current behavior includes accessible Target rows with status descriptions, Space-to-toggle without Enter-to-toggle, disabled controls leaving the tab order, scrolling focused rows into view, system light/dark/high-contrast colors, and Windows text scaling. A replacement must preserve those behaviors and reduce the total implementation needed to support them.

The pinned GPUI revision provides layout, scrolling, focus, and accessibility primitives; it does not include ready-made checkbox, button, or scrollbar controls. Zed's separate `ui` crate supplies buttons and scrollbars, but its [dependency list](https://github.com/zed-industries/zed/blob/933d8d93819c749a607e561883855a9b95c79cea/crates/ui/Cargo.toml) and implementations rely on Zed's theme, icon, and component infrastructure. Importing that stack for this screen is not a demonstrated simplification.

[GPUI Kit's behavior primitives](https://github.com/longbridge/gpui-kit/blob/main/docs/ARCHITECTURE.md) are a closer conceptual fit: they separate interaction from product styling. Its [current manifest](https://github.com/longbridge/gpui-kit/blob/main/Cargo.toml) uses the separately packaged `gpui-pre` snapshots. Cleaner uses the git-pinned Zed `gpui` package; their Rust types cannot be interchanged directly. Adoption would require a dependency migration and validation of Cleaner’s Windows startup, packaging, appearance, and accessibility, beyond replacing individual controls.

Windows standard controls supply much of their own [accessibility support](https://learn.microsoft.com/en-us/windows/win32/winauto/accessibility-best-practices). They are a credible alternative for a native screen, but replacing or mixing the current rendering with native controls requires new window, layout, and appearance integration. This evaluation does not establish that the resulting screen would be smaller while preserving all current behavior.

Retain the current controls for this refactor. Both buttons share one implementation of focus, role, and disabled behavior; rendering, presentation, and interaction are separated without adding a new UI framework or generic control system. A broader control migration has not been prototyped or approved by this evaluation.
