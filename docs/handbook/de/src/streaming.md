# Welt-Streaming

Dieses Kapitel folgt einer Zelle von der Platte bis auf den Bildschirm.
Alles lebt in [`re_world`](../../rustdoc/re_world/index.html).

## Raster und Clipmap

Die Welt ist ein unendliches Raster gleich großer quadratischer Zellen
(`CellKey { level, x, y }`). Um die Kamera liegen mehrere
**Clipmap-Ebenen**: Ebene 0 nutzt die Basiszellgröße, jede äußere
Ebene verdoppelt sie. Eine Zelle gehört zur feinsten Ebene, die sie
noch abdeckt — Speicher kostet sie also einmal, nicht pro Ebene.

```rust
use re_world::{Clipmap, ClipmapConfig};
let mut clip = Clipmap::new(ClipmapConfig { base_cell: 64.0, radius: 2, levels: 3 });
clip.set_center(glam::Vec2::new(500.0, 0.0)); // → ClipDiff der neuen Zellen
```

## Der Streamer und sein Budget

Der `Streamer` hält Zellen innerhalb eines Byte-Budgets resident. Der
Bedarf kommt als `(CellKey, Priority)`; Lasten laufen durch eine
`CellSource` — entweder `MockSource` (deterministisch, für Tests) oder
`AsyncSource` (echte Datei-IO auf Worker-Threads, Dekodierung durch
eine `CellCodec`-Closure). Verdrängte Zellen bleiben in einer LRU
liegen, bis das Budget ihre Bytes wirklich braucht — Umdrehen lädt
also nicht neu.

## GPU-Handoff

Der `CellHandoff` ist eine Triple-Buffer-Pipeline zwischen Streamer
und GPU: pro Frame wird höchstens ein Byte-Kontingent hochgeladen,
frisch geladene Zellen werden erst sichtbar, wenn ihr Upload retiret
ist, und freigegebene Zellen laufen mit Karenzzeit aus. Die
Render-Schleife blockiert nie für IO — deshalb ist „kein Ladebildschirm“
eine Eigenschaft der Pipeline und kein Glück.

## Quality-Tiers und die Ego-Übergabe

Jede Instanz trägt eine Stufe `T0..T4`, nach Kameradistanz mit
Hysterese und Mindest-Verweilzeit eingestuft — eine laufende Kamera
bringt die Stufen nicht zum Flackern. Die **Ego-Übergabe**
(`re_world::ego::EgoHandoff`) ist das Leitfeature: Die Kamera gleitet
auf einer Smoothstep-Blende vom Strategie-Anker zur Einheit, der
Eingabe-Besitz wandert mit dem Ende der Blende, und die Welt-Pipeline
merkt nichts davon — dieselben Zellen, dasselbe Budget, kein
Neuaufbau.

## HLOD und IO-Telemetrie

Jenseits der Clipmap-Reichweite zeigt `HlodSet` vorgebackene
Fernfeld-Cluster statt Nichts; die Übernahmezone nutzt Hysterese und
einen Fade-out-Ring. Misst die `IoTelemetry` derweh echte Lade-Latenz
und Durchsatz — auf langsamer Platte werten die Stufenregeln früher ab
(`PartitionConfig::tiers_adaptive`).

## Ansteuern

```rust
let mut partition = WorldPartition::new(cfg, source);
let id = partition.spawn(unit_pos, Some(camera));
loop {
    let delta = partition.update(camera, dt); // geladen/verdrängt/Stufenwechsel
    // delta.loaded in den CellHandoff speisen, rendern, wiederholen
}
```

Der `stream-demo` ist genau diese Schleife als Laufprogramm.
