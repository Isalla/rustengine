# World Streaming

This chapter follows one cell from disk to screen. Everything lives in
[`re_world`](../../rustdoc/re_world/index.html).

## Grid and clipmap

The world is an infinite grid of equal-sized square cells
(`CellKey { level, x, y }`). Around the camera sit several **clipmap
levels**: level 0 uses the base cell size, each outer level doubles it.
A cell belongs to the finest level that still covers it, so coverage
costs memory once, not per level.

```rust
use re_world::{Clipmap, ClipmapConfig};
let mut clip = Clipmap::new(ClipmapConfig { base_cell: 64.0, radius: 2, levels: 3 });
clip.set_center(glam::Vec2::new(500.0, 0.0)); // → ClipDiff of entered cells
```

## The streamer and its budget

`Streamer` keeps cells resident within a byte budget. Demand comes in
as `(CellKey, Priority)`; loads run through a `CellSource` — either
`MockSource` (deterministic, for tests) or `AsyncSource` (real file IO
on worker threads, decoding through a `CellCodec` closure). Evicted
cells linger in an LRU until the budget truly needs their bytes, so
turning around doesn't reload.

## GPU handoff

`CellHandoff` is a triple-buffered pipeline between streamer and GPU:
each frame uploads at most a byte quota, freshly uploaded cells become
visible only after their upload retires, and released cells go back
after a grace period. The render loop never blocks on IO — that is what
makes "no loading screen" a property of the pipeline, not of luck.

## Quality tiers and the ego handoff

Every instance carries a tier `T0..T4` graded by distance to the camera
with hysteresis and a minimum dwell time, so a walking camera can't
make tiers flicker. The **ego handoff** (`re_world::ego::EgoHandoff`)
is the signature move: the camera glides along a smoothstep blend from
the strategy anchor to the unit, input ownership transfers when the
blend lands, and the world pipeline never notices — same cells, same
budget, no rebuild.

## HLOD and IO telemetry

Beyond the clipmap's reach, `HlodSet` shows baked far-field summary
clusters instead of nothing; take-over zones use hysteresis and a
fade-out ring. Meanwhile `IoTelemetry` measures real load latency and
throughput — on a slow disk the tier rules degrade earlier
(`PartitionConfig::tiers_adaptive`).

## Driving it

```rust
let mut partition = WorldPartition::new(cfg, source);
let id = partition.spawn(unit_pos, Some(camera));
loop {
    let delta = partition.update(camera, dt); // loaded/evicted/tier changes
    // feed delta.loaded into CellHandoff, render, repeat
}
```

`stream-demo` is the runnable version of exactly this loop.
