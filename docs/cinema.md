# Cinema editor — cutscenes inside the game (concept, M7)

English | [Deutsch](cinema.de.md)

An editor mode in which users and the AI **build cutscenes**: the camera
flies over the map, units act out a scene, a door closes, music kicks
in — all inside the game/editor, without an external video editor.

## Building block: `Sequence` as an engine asset

The existing animation layer (`re-core::animation` with `Channel`,
`KeyValue`, `Interpolation`, `Target::Node/Joint`) is already half of
the timeline. A cinema asset is a **collection of tracks** writing to
the same scene math:

| Track type | Target | Example |
|---|---|---|
| Transform | node | marine runs to the hatch (position + rotation on a curve) |
| Camera | own path | spline with look-at point + FOV animation (dolly/crash zoom) |
| Property | node/material | hatch `open: 0→1` (uses the sockets from the tank case!) |
| Visibility | node | rubble fading in, UI fading out |
| Audio | mix | voice line + music swell at time X |
| FX | emitter | dust cloud on impact, time signal to gameplay |
| Dialogue | overlay | text/speech with lip-sync trigger (later) |

Important: **evaluation is a pure function** `eval(sequence, t) → scene
state`. That makes everything scrubbable, rewindable and deterministic —
the precondition for the editor timeline AND for offline rendering by
the AI.

## Editor (M7)

Timeline panel below the 3D viewport (the viewport shows a live scrub):
tracks as a list, keyframes as dopesheet/curves, camera track with
spline handles — the same selection tools as in the mesh editor (tank
case): whatever you can click, you can key. Playhead, loop ranges,
blend zones between sequences.

## In the game

`re-cinema` player: the game pauses logic, the player hands over
control, the sequence runs, signals go to gameplay (dialog options,
spawn events). The cutscene rig (briefing camera) and the in-engine
cutscene are the same code — no "baked video" needed.

## AI operation (capabilities, from M6)

* `cinema.create(name)` / `cinema.add_camera(keys)` /
  `cinema.key(node, prop, t, value)`
* `cinema.from_script("The marine reaches the hatch and closes it")`
  — the AI uses scene knowledge (`SceneDigest`) + sockets + animations
  and builds the tracks itself; manual correction in the editor any
  time.
* `cinema.preview(seq, t)` → offscreen frames (re-wgpu already does
  this) back into the AI conversation: the AI **sees** its cutscene.
