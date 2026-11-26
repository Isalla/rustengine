# Editor UI — presets, identity, reference set

[Deutsch](editor-ui.de.md)

This document is the working spec for the editor frontend. It records
the preset decision ([ADR-0001](adr/0001-modular-editor-frontend.md)),
the identity of the engine's own default look, the inventory of the
delivered UE5 reference set, and the gaps still open in it.

## 1. Two faces, one codebase

A frontend preset is **data, not code**: a layout manifest (which
panels dock where), control→capability bindings, and a theme (colors,
metrics, labels). The widget set (viewport, outliner, inspector,
console, content browser) and the capability registry are shared by
every preset.

The decision (product, 2026-11-26): the engine ships **its own
`default` preset** as its face, and the **`ue5` preset as an optional
look** for users arriving from Unreal. This keeps the engine's
identity its own — the UE5 layout is a variant, not the brand.

## 2. Identity of the `default` preset

| Aspect | `default` (own face) | `ue5` (optional) |
| --- | --- | --- |
| Chrome | slim command bar + **command palette** (Ctrl+Space) instead of a classic menu bar | classic menu bar, nine menus, German labels |
| Colors | warm dark grays, **rust orange** accent | UE5 "Dunkel": near-black cold grays, blue accent (exact values in the theme JSON) |
| Assistant | **assistant dock as a first-class panel** — the M11 double-addressability doctrine made visible | assistant available, not front stage |
| Panels | same shared widgets, own arrangement from the manifest | UE5 arrangement from the screenshots |

The docking system, outliner, inspector, viewport host and console are
built **once**; a preset only re-arranges and re-skins them.

## 3. Reference set inventory (delivered 2026-11-26)

Location: `.share/referenz/ue5-ui/` (outside the repo — reference
material, not a shipped asset).

| Item | Specifies |
| --- | --- |
| `UE5_1.png` | full editor layout + File menu: menu structure, shortcuts column, disabled items, tooltip style, content drawer bar |
| `UE5_2.png` | Edit menu (history, clipboard, configuration entries) |
| `UE5_3_*.png` | Window menu + submenus — the openable-panel set (Details 1–4, Outliner, Content Browser, Output Log, …), layout load/save |
| `UE5_4_*.png` | Tools menu + submenus (incl. search-in-submenu pattern) |
| `UE5_5_*.png` | Create menu — radio and check items inside menus |
| `UE5_6_*.png` | Platforms menu — two-level submenus, disabled submenu info text |
| `UE5_7_*.png` | Select menu + submenus |
| `UE5_8_*.png` | Actor menu + submenus (long labels, truncation) |
| `UE5_9.png` | Help menu |
| `UE5_10.png` | full editor populated: toolbar (play controls, dropdowns, transform tools, settings), outliner with search/filter/multi-select, details panel (tabs, sections, XYZ rows, key/value info, checkboxes), class-picker dialog with open dropdown, bottom tab bar, status bar, viewport gizmo |
| `editor-einstellungen.png` | settings dialog: sidebar tree, search, filter chips, sections, checkboxes, buttons, spinners, lock icons |
| `projekteinstellungen.png` | settings dialog variant: category tree, dropdown fields, array-element rows |
| `plugins.png` | plugin browser: sidebar with counts, rows with checkbox/icon/badge/version |
| `…_Aussehen_Dunkel.json` | **exact UE5 theme colors** (`EStyleColor::*`, linear RGBA) — the `ue5` theme file seeds from these |
| `EditorPerProjectUserSettings.ini`, `Editor Settings Backup….ini`, `Project Settings Backup….ini` | real editor metrics, shortcut maps, dock layout — source for spacing/sizes |

Interaction states the set already proves: hover/selected (blue),
disabled (gray), tooltip (white box), submenu arrow, section
separators with small-caps labels, text truncation with ellipsis,
thin scrollbars, tab close button, menu search field.

## 4. Gaps — still missing from the reference set

Priority order; the toolkit can start without them, the `ue5` preset
polish cannot finish without the first block:

1. **Content Browser** opened — grid view, list view, sidebar tree,
   collection bar (only its tab button is shown).
2. **Output Log** expanded — colored lines, filter dropdown, search.
3. **Context menus** — right-click in viewport, outliner, content
   browser, and on a details property.
4. **Toolbar dropdowns opened** — view mode (Beleuchtet), Perspektive,
   Show (eye), Produktionen, camera speed.
5. **Place Actors** panel (Actors platzieren).
6. **Details interaction states** — open enum dropdown, color picker,
   text field in edit (caret), value drag, reset-icon hover.
7. **Console / command palette** with autocomplete list.
8. **Floating panel + dock drop hints** — detached title bar, tab drag
   ghost, drop-target lines.
9. **Message box** (Yes/No/Cancel) and a progress dialog.
10. **Content Drawer expanded** (the shelf opened).
11. Play-mode overlay in the viewport (optional).

Icons: the screenshots show menu icons too small to extract as
vectors. Per ADR-0001 icon fidelity is art effort carried by the
theme — the toolkit ships simple drawn glyphs; a matching icon set can
replace them later without code changes.

## 5. P1 scope (the build that starts now)

- **Widget toolkit on `re-render`**: draw list (rects, rounded rects,
  lines, scissors), text via font atlas (`ab_glyph`, Inter — SIL OFL,
  bundled under `assets/fonts/`), input routing through
  `re-platform` events, immediate-mode widget layer.
- **Theme file**: colors/metrics/labels as data; the `default` theme
  (warm grays + rust orange) ships first, the `ue5` theme seeds from
  the JSON above.
- **Layout manifest**: panels, dock sides, sizes, titles as data;
  docking (split/tab/float/resize) is the shared system.
- **Default layout**: command bar, viewport center, outliner left,
  inspector right, console bottom, assistant dock.
- Every control binds a capability name (`EditorContext::call`) —
  never engine internals (double addressability).
