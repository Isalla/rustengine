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
| `UE5_11.png` | Content Browser opened (grid view): sidebar tree (Favoriten/Alle/Sammlungen), collection bar, breadcrumbs, asset toolbar, view toggles, selection status bar; Outliner with a real tree (eye visibility icons, name search, filter button, column header, bottom level tabs) |
| `UE5_12_outputlog.png` | Output Log expanded: colored lines (warning yellow, error red, info white), category filter dropdown, "Enthaltungen anzeigen" checkbox, search field, copy/clear icons |
| `UE5_13.png` | Content Browser list-view toggle + asset hover tooltip (thumbnail + metadata card) |
| `UE5_14.png` | Content Browser file list with **inline rename** (text field in edit, selection highlight) and status-colored rows |
| `UE5_15.png` | Viewport right-click **context menu**: search field, small-caps section labels, shortcut column, checkmark items, disabled items, submenu arrows |
| `UE5_16.png` | **Place Actors** panel: class search, category list, left icon rail with flyout, panel close button |
| `UE5_17.png` | opened toolbar dropdown (view mode) + white hover tooltip |
| `UE5_18.png` | viewport-tab context menu (checkmarks, disabled entries, section labels) |
| `Bewegungseditor*.png` | sequencer reference for M11: track view (ruler, playhead, key diamonds, camera track), tool rail with icon+text flyout grids (selected = blue), modal message box ("NumLock aktiviert") |
| `UE5_19.png` | **detached (floating) panel**: Content Browser torn out of the editor — own OS title bar with min/max/close, own tab row, floating beside the main window over the desktop; Output Log expanded underneath |
| `cmd.png` | console type selector (Cmd / Python / Python (REPL)) with checkmark — the console entry point |
| `Editordiagnose.png` | modal dialog window: title bar with close button, tab pills (Alles anzeigen / Nur Warnungen), gear menu, table with column headers (Aktuell / Expected / Fehlschläge / Notify), section rows, checkbox column, status-colored values; left icon rail with text buttons (selected = blue) |
| `Trace-Objekt_1/2.png` | rich toolbar dropdown: search field, small-caps section labels, radio items (filled dot), checkbox items, shortcut column, disabled items, **focused text input (blue border)**, combo box, **submenu flyout** with hover highlight (blue), white tooltip anchored above the menu |
| `Zen-Server.png` | status-bar dropdown with section labels + second-level submenu (icon entries), hover highlight |
| `revisionskontrolle.png` | status-bar menu with a disabled entry (prohibiting icon) and tooltip anchored above the menu |
| `UE5_20_farbpicker.png` | **Details color picker** opened from a property row: color wheel, value + saturation sliders, RGB sliders, HSV sliders, hex field, preset swatch grid, eyedropper/invert icons, per-channel reset arrows, OK / Cancel |
| `UE5_21_enumdropdown.png` | **enum dropdown opened** (viewport projection mode): current item marked with a checkmark, shortcut column, small-caps section labels, live numeric fields inside the menu, **submenu flyouts** with arrows |
| `UE5_22_playoverlay.png` | **play mode**: viewport switched to the game view, toolbar play controls replaced by pause/skip/stop, on-screen overlay text ("Spieler [U] wird gestartet" + hint line), and a background-task notification with spinner ("Offener Vorgang: Lichtbuild bearbeiten …") |
| `UE5_23_console_autocomplete.png` | **console autocomplete list**: 20 matching cvars above the focused input, each with a category letter icon, common prefix kept, differing tail highlighted |

Interaction states the set now proves: hover/selected (blue),
disabled (gray, with prohibiting icon), tooltip (white box, also
anchored above menus; dark variant with icon), submenu arrow + opened
flyout, section separators with small-caps labels, text truncation with
ellipsis, thin scrollbars, tab close button, menu search field,
context-menu search, inline text edit with selection, focused text
input (accent border), radio items, checkbox items and checkbox table
columns, combo box, checkmark menu items, colored log lines, asset
hover cards, modal message box, modal dialog window with table,
tab pills, content-drawer tile shelf, **detached floating window**,
**color picker dialog**, **enum dropdown with checkmark + shortcuts +
live fields**, **console autocomplete list**, **play-mode overlay text
and background-task notification**.

## 4. Gaps — still missing from the reference set

Closed on 2026-11-27 (second capture pass, `.share/referenz/ue5-ui/`):
console autocomplete (`UE5_23_console_autocomplete.png`), Details color
picker (`UE5_20_farbpicker.png`), enum dropdown (`UE5_21_enumdropdown.png`),
play-mode overlay (`UE5_22_playoverlay.png`), detached floating panel
(`UE5_19.png`).

What is left, and why it is the end of what a screenshot pass can add:

1. **Dock drop hints while dragging** — the detached title bar is proven
   (`UE5_19`), the drop-target lines are not reachable: the automation
   drag is atomic (down→move→up without releasing the game thread), so
   no frame exists between press and release to capture.
2. **Details value drag** (scrubbing a number) — same atomic-input cause;
   the drag itself is the state to photograph.
3. **Progress dialog** — no side-effect-free trigger exists. A Live
   Coding compile reports `CompileNotStarted` (disabled in this project),
   a cvar change recompiles from cache with no visible bar, and the
   remaining triggers (save-all, lighting build, plugin install) all
   mutate the project. The background-task notification with spinner in
   `UE5_22_playoverlay.png` is the closest honest substitute.

### 4.1 What the UE5.8 MCP surface cannot reach (capture doctrine)

Recorded so a future pass does not rediscover them:

- **Widget refs are session-local.** Every MCP session renumbers them,
  so resolve → click → capture must happen inside one session.
- **The Details panel is only in the accessibility tree when its tab is
  active and something is selected.** An inactive tab contributes
  nothing, which reads as "the panel is inaccessible" when it is not.
- **Small enums render as segmented buttons, not dropdowns** (Actor
  mobility: Statisch/Stationär/Beweglich inline). A dropdown needs an
  enum with enough values — the projection-mode combo shows the real
  dropdown form.
- **Property rows are single flattened nodes** with no child widgets, so
  a center click can land on the label and raise a tooltip instead of
  the editor. Value cells need a coordinate click.
- **Modal dialogs ignore synthetic clicks** (the color picker's Cancel
  never fired). They are real OS windows: close them with `WM_CLOSE`.
- **Transient popups die when captured.** The image tool takes focus and
  closes menus; grab the screen region with GDI instead.
- **The menu-bar command search is not in the accessibility tree**, and
  its result popup does not survive automation-driven typing. Ctrl+Space
  is unbound in this build. The console autocomplete
  (`UE5_23_console_autocomplete.png`) is the reachable equivalent.
- **No absolute-coordinate click and no context-menu key** in the
  inspector toolset; anything needing those is out of reach by design.

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

## 6. Raster conventions (the CPU core is the truth, the GPU mirrors it)

The UI draws through one draw-list (`re_render::ui::DrawList`); the
CPU reference rasterizer (`rasterize`) and the GPU pass
(`re_wgpu::ui::UiGpu`, shader `ui.wgsl`) must produce the same pixels —
same doctrine as the shadow core. `tests/ui_gpu.rs` gates this: a
representative scene (fills, snapped lines, nested clips to the edge,
overlapping alpha blends, two-font glyph text with umlauts and
em-dash) diffs at **max 1/255 per channel, zero pixels beyond** (the
1 LSB is RGBA16F intermediate storage; the gate allows exactly that).

The shared conventions that make the mirror exact:

- **Pixel-snapped geometry**: glyph quads snap to the integer pixel
  grid at layout time (pen stays fractional, quad edges do not). All
  edges are then integers, pixel centers (x.5) never land on an edge,
  and no rasterizer's subpixel quantization can flip a coverage
  column. Glyph sampling becomes 1:1 — each target pixel hits exactly
  one atlas texel, bilinear degenerates to a texel read.
- **Integer scissor**: `scissor_rect()` floors/ceils the float clip to
  a pixel box; the CPU rasterizer and `set_scissor_rect` consume the
  same box.
- **Manual bilinear on the GPU**: the shader filters the atlas with
  `textureLoad` in the CPU's exact operation order — hardware filter
  weights are fixed-point and would diverge by LSBs.
- **Linear float accumulation**: the GPU renders to RGBA16F (premultiplied
  `ONE, ONE_MINUS_SRC_ALPHA` == the CPU blend formula) and quantizes
  to 8 bits only after readback, like the CPU reference does.
- Text colors are linear RGBA; the window path composites onto the
  scene after tone-mapping (the UI pass itself never sRGB-encodes).
