//! Welt-Capabilities (M3): Streaming und Szenen-Abfragen als Agent-Werkzeuge.
//!
//! Die `WorldPartition` ist der Kopflos-Kern — der Agent darf sie trotzdem
//! anfassen, denn genau das ist die Regel der doppelten Adressierbarkeit:
//! Was der Editor zeigt, steuert die KI über dieselben Registry-Werkzeuge.
//! [`register_world_tools`] hängt die Abfragen und Eingriffe an eine
//! bestehende [`CapabilityRegistry`]; der Renderer bleibt außen vor, es
//! geht nur um Welt-Zustand (Zellen, Instanzen, Kamera-Anker, Telemetrie).
//!
//! Die Partition läuft in einem `Mutex` hinter einem `Arc`: `call` ist
//! `&self`, also kapseln die Runner ihre Innere Mutabilität selbst — der
//! Editor hält denselben Handle und sieht jede Änderung sofort.
//!
//! Das Werkzeug `world.tick` lässt der Agent die Welt selbst weiterschieben
//! (deterministisch, gleiche `dt`-Folge → gleiche Ereignisse). Ohne Tick
//! bleiben die Abfragen reine Lesezugriffe auf den letzten Stand.

use std::sync::{Arc, Mutex, MutexGuard};

use glam::Vec2;
use serde_json::json;

use crate::capability::{CapabilityRegistry, ToolSpec};
use re_world::{CellSource, WorldPartition};

/// Welt plus der Kamera-Anker, den die Werkzeuge setzen. Die Partition
/// selbst kennt keine Kamera (Kopflos-Kern) — für den Agenten gehört sie
/// trotzdem zum Adressierbaren.
pub struct WorldSession<S: CellSource> {
    pub partition: WorldPartition<S>,
    /// Kamerapunkt für [`tick`](WorldPartition::update) und Abfragen.
    pub camera: Vec2,
}

impl<S: CellSource> WorldSession<S> {
    pub fn new(partition: WorldPartition<S>, camera: Vec2) -> Self {
        Self { partition, camera }
    }
}

/// Shared-Handle für die Registry-Runner (Editor und Agent halten denselben).
pub type SharedWorld<S> = Arc<Mutex<WorldSession<S>>>;

/// Sperrt die Welt; vergiftete Mutexe (Panik eines anderen Runners) werden
/// zu einem sauberen Fehler statt zur nächsten Panik.
fn lock<S: CellSource>(world: &SharedWorld<S>) -> Result<MutexGuard<'_, WorldSession<S>>, String> {
    world.lock().map_err(|_| "Welt-Sperre vergiftet — vorheriger Aufruf gepanikt".to_string())
}

fn arg_f32(args: &serde_json::Value, key: &str) -> Result<f32, String> {
    args.get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .ok_or_else(|| format!("Parameter '{key}' fehlt (Zahl erwartet)"))
}

fn arg_usize(args: &serde_json::Value, key: &str) -> Result<usize, String> {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .ok_or_else(|| format!("Parameter '{key}' fehlt (Ganzzahl erwartet)"))
}

/// Zahl der Liste, ab der die Antwort gekürzt wird (Prompt-Atmung).
const LIST_CAP: usize = 512;

/// Hängt die Welt-Werkzeuge an die Registry. Bestehende Namen werden
/// ersetzt (Registrieren ist idempotent über Neuregistrierung).
pub fn register_world_tools<S>(reg: &mut CapabilityRegistry, world: SharedWorld<S>)
where
    S: CellSource + Send + 'static,
    S::Payload: Send,
{
    // ── Abfragen ──────────────────────────────────────────────────────
    {
        let w = Arc::clone(&world);
        reg.register(ToolSpec::simple("world.stats", "Welt-Überblick: Zeit, Kamera, Zell-Bestand, IO-Telemetrie, HLOD, Instanzen"), move |_| {
            let s = lock(&w)?;
            let p = &s.partition;
            let t = p.streamer().telemetry();
            let cm = p.clipmap().config();
            Ok(json!({
                "zeit": p.time(),
                "kamera": { "x": s.camera.x, "z": s.camera.y },
                "clipmap": {
                    "ebenen": cm.levels,
                    "basiszelle": cm.base_cell,
                    "reichweite": cm.outer_radius(),
                    "aktive_zellen": p.clipmap().cells().len(),
                },
                "streamer": {
                    "resident_zellen": p.streamer().resident().len(),
                    "resident_bytes": p.streamer().resident_bytes(),
                },
                "io_telemetrie": {
                    "lasten": t.samples(),
                    "fehler": t.failures(),
                    "latenz_ms": t.latency_ewma() * 1000.0,
                    "durchsatz_mibs": t.throughput_ewma() / (1024.0 * 1024.0),
                    "gesundheit": t.health(),
                },
                "hlod": {
                    "cluster": p.hlod().len(),
                    "sichtbar": p.hlod().shown().len(),
                },
                "instanzen": p.instances().len(),
            }))
        });
    }
    {
        let w = Arc::clone(&world);
        reg.register(
            ToolSpec::new(
                "world.cells.resident",
                "Residente Zellen auflisten (optional nach Ebene gefiltert)",
                json!({ "typ": "object", "felder": { "ebene": { "typ": "integer" } } }),
            ),
            move |args| {
                let s = lock(&w)?;
                let level = args.get("ebene").and_then(|v| v.as_u64()).map(|v| v as u8);
                let mut out = Vec::new();
                let mut total = 0usize;
                for (key, bytes) in s.partition.streamer().resident() {
                    if level.is_some_and(|l| key.level != l) {
                        continue;
                    }
                    total += 1;
                    if out.len() < LIST_CAP {
                        out.push(json!({ "ebene": key.level, "x": key.x, "z": key.y, "bytes": bytes }));
                    }
                }
                Ok(json!({ "zellen": out, "anzahl": total, "gekuerzt": total > out.len() }))
            },
        );
    }
    {
        let w = Arc::clone(&world);
        reg.register(
            ToolSpec::new(
                "world.cell.state",
                "Streaming-Zustand einer Zelle: queued|loading|ready|none",
                json!({ "typ": "object", "felder": {
                    "ebene": { "typ": "integer" }, "x": { "typ": "integer" }, "z": { "typ": "integer" }
                } }),
            ),
            move |args| {
                let s = lock(&w)?;
                let level = args
                    .get("ebene")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u8)
                    .ok_or("Parameter 'ebene' fehlt (Ganzzahl erwartet)")?;
                let key = re_world::CellKey {
                    level,
                    x: arg_usize_i64(&args, "x")?,
                    y: arg_usize_i64(&args, "z")?,
                };
                let state = match s.partition.streamer().state_of(key) {
                    Some(re_world::SlotState::Queued) => "queued",
                    Some(re_world::SlotState::Loading) => "loading",
                    Some(re_world::SlotState::Ready) => "ready",
                    None => "none",
                };
                Ok(json!({ "zustand": state }))
            },
        );
    }
    {
        let w = Arc::clone(&world);
        reg.register(
            ToolSpec::new(
                "world.hlod.list",
                "HLOD-Cluster auflisten (nur sichtbare auf Wunsch)",
                json!({ "typ": "object", "felder": { "nur_sichtbare": { "typ": "boolean" } } }),
            ),
            move |args| {
                let s = lock(&w)?;
                let shown_only = args
                    .get("nur_sichtbare")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let hl = s.partition.hlod();
                let mut out = Vec::new();
                for c in hl.shown() {
                    if out.len() >= LIST_CAP {
                        break;
                    }
                    out.push(json!({
                        "x": c.x, "z": c.y,
                        "quellzellen": hl.source_cells(c).unwrap_or(0),
                    }));
                }
                if !shown_only {
                    // Vollständige Liste: gezeigte sind oben schon erfasst.
                    for cell in hl.cells() {
                        if out.len() >= LIST_CAP {
                            break;
                        }
                        if !hl.is_shown(cell) {
                            out.push(json!({
                                "x": cell.x, "z": cell.y,
                                "quellzellen": hl.source_cells(cell).unwrap_or(0),
                            }));
                        }
                    }
                }
                Ok(json!({ "cluster": out, "anzahl": hl.len() }))
            },
        );
    }
    {
        let w = Arc::clone(&world);
        reg.register(ToolSpec::simple("world.instances.list", "Alle Instanzen mit Position und Qualitätsstufe"), move |_| {
            let s = lock(&w)?;
            let out: Vec<_> = s
                .partition
                .instances()
                .iter()
                .enumerate()
                .take(LIST_CAP)
                .map(|(id, inst)| {
                    json!({
                        "id": id,
                        "x": inst.position.x,
                        "z": inst.position.y,
                        "stufe": format!("{:?}", inst.tier.tier),
                    })
                })
                .collect();
            Ok(json!({ "instanzen": out, "anzahl": s.partition.instances().len() }))
        });
    }

    // ── Eingriffe ─────────────────────────────────────────────────────
    {
        let w = Arc::clone(&world);
        reg.register(
            ToolSpec::new(
                "world.camera.set",
                "Kamera-Anker setzen (wirkt auf den nächsten world.tick)",
                json!({ "typ": "object", "felder": { "x": { "typ": "number" }, "z": { "typ": "number" } } }),
            ),
            move |args| {
                let mut s = lock(&w)?;
                s.camera = Vec2::new(arg_f32(&args, "x")?, arg_f32(&args, "z")?);
                Ok(json!({ "kamera": { "x": s.camera.x, "z": s.camera.y } }))
            },
        );
    }
    {
        let w = Arc::clone(&world);
        reg.register(
            ToolSpec::new(
                "world.tick",
                "Welt einen Schritt weiterschieben (dt Sekunden); liefert die Ereigniszahlen des Schritts",
                json!({ "typ": "object", "felder": { "dt": { "typ": "number" } } }),
            ),
            move |args| {
                let mut s = lock(&w)?;
                let dt = arg_f32(&args, "dt")?;
                if !(dt.is_finite() && dt >= 0.0) {
                    return Err("dt muss eine endliche, nicht-negative Zahl sein".into());
                }
                let delta = {
                    let cam = s.camera; // Kopie: update braucht &mut partition
                    s.partition.update(cam, dt)
                };
                Ok(json!({
                    "geladen": delta.loaded.len(),
                    "verdraengt": delta.evicted.len(),
                    "fehlgeschlagen": delta.failed.len(),
                    "stufenwechsel": delta.tier_changed.len(),
                    "hlod_uebernommen": delta.hlod.shown.len(),
                    "hlod_zurueck": delta.hlod.hidden.len(),
                }))
            },
        );
    }
    {
        let w = Arc::clone(&world);
        reg.register(
            ToolSpec::new(
                "world.instance.spawn",
                "Instanz in die Welt setzen; liefert ihre id",
                json!({ "typ": "object", "felder": { "x": { "typ": "number" }, "z": { "typ": "number" } } }),
            ),
            move |args| {
                let mut s = lock(&w)?;
                let pos = Vec2::new(arg_f32(&args, "x")?, arg_f32(&args, "z")?);
                let cam = Some(s.camera); // Kopie vor dem &mut-Zugriff
                let id = s.partition.spawn(pos, cam);
                Ok(json!({ "id": id, "x": pos.x, "z": pos.y }))
            },
        );
    }
    {
        let w = Arc::clone(&world);
        reg.register(
            ToolSpec::new(
                "world.instance.move",
                "Instanz umsetzen (Stufenneubewertung im nächsten Tick)",
                json!({ "typ": "object", "felder": {
                    "id": { "typ": "integer" }, "x": { "typ": "number" }, "z": { "typ": "number" }
                } }),
            ),
            move |args| {
                let mut s = lock(&w)?;
                let id = arg_usize(&args, "id")?;
                let pos = Vec2::new(arg_f32(&args, "x")?, arg_f32(&args, "z")?);
                if s.partition.set_instance_position(id, pos) {
                    Ok(json!({ "ok": true, "id": id }))
                } else {
                    Err(format!("keine Instanz mit id {id}"))
                }
            },
        );
    }
    {
        let w = Arc::clone(&world);
        reg.register(
            ToolSpec::new(
                "world.instance.remove",
                "Instanz aus der Welt nehmen",
                json!({ "typ": "object", "felder": { "id": { "typ": "integer" } } }),
            ),
            move |args| {
                let mut s = lock(&w)?;
                let id = arg_usize(&args, "id")?;
                match s.partition.despawn(id) {
                    Some(inst) => Ok(json!({ "ok": true, "x": inst.position.x, "z": inst.position.y })),
                    None => Err(format!("keine Instanz mit id {id}")),
                }
            },
        );
    }
}

/// Ganzzahl-Parameter als i64 (Zellkoordinaten sind negativ möglich).
fn arg_usize_i64(args: &serde_json::Value, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("Parameter '{key}' fehlt (Ganzzahl erwartet)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use re_world::{ClipmapConfig, MockSource, PartitionConfig, StreamConfig, TierPolicy};
    use serde_json::json;

    fn session() -> SharedWorld<MockSource> {
        let partition = WorldPartition::new(
            PartitionConfig {
                clipmap: ClipmapConfig { base_cell: 64.0, radius: 2, levels: 3 },
                stream: StreamConfig { budget_bytes: 100_000, max_inflight: 64 },
                tiers: TierPolicy::default(),
                tiers_adaptive: false,
            },
            MockSource::new(0.0, 100),
        );
        Arc::new(Mutex::new(WorldSession::new(partition, Vec2::ZERO)))
    }

    fn rig() -> CapabilityRegistry {
        let mut reg = CapabilityRegistry::new();
        register_world_tools(&mut reg, session());
        reg
    }

    #[test]
    fn alle_werkzeuge_sind_registriert() {
        let reg = rig();
        for name in [
            "world.stats",
            "world.cells.resident",
            "world.cell.state",
            "world.hlod.list",
            "world.instances.list",
            "world.camera.set",
            "world.tick",
            "world.instance.spawn",
            "world.instance.move",
            "world.instance.remove",
        ] {
            assert!(reg.has(name), "fehlt: {name}");
        }
    }

    #[test]
    fn spawn_bewegen_entfernen_ueber_die_registry() {
        let reg = rig();
        let id = reg.call("world.instance.spawn", json!({ "x": 10.0, "z": 20.0 })).unwrap()["id"]
            .as_u64()
            .unwrap();
        let list = reg.call("world.instances.list", json!({})).unwrap();
        assert_eq!(list["anzahl"], json!(1));
        assert_eq!(list["instanzen"][0]["x"], json!(10.0));
        reg.call("world.instance.move", json!({ "id": id, "x": 30.0, "z": 40.0 })).unwrap();
        let list = reg.call("world.instances.list", json!({})).unwrap();
        assert_eq!(list["instanzen"][0]["x"], json!(30.0));
        reg.call("world.instance.remove", json!({ "id": id })).unwrap();
        assert_eq!(reg.call("world.instances.list", json!({})).unwrap()["anzahl"], json!(0));
        // Ungültige id → Fehler, keine Panik.
        assert!(reg.call("world.instance.move", json!({ "id": 99, "x": 0.0, "z": 0.0 })).is_err());
    }

    #[test]
    fn kamera_und_tick_bewegen_die_welt() {
        let reg = rig();
        reg.call("world.camera.set", json!({ "x": 500.0, "z": 0.0 })).unwrap();
        let stats = reg.call("world.stats", json!({})).unwrap();
        assert_eq!(stats["kamera"]["x"], json!(500.0));
        // Erster Tick: Clipmap zieht nach und startet die Lasten; fertig
        // werden sie im zweiten (MockSource-Latenz 0 → sofort bereit).
        let t1 = reg.call("world.tick", json!({ "dt": 0.016 })).unwrap();
        let t2 = reg.call("world.tick", json!({ "dt": 0.016 })).unwrap();
        let geladen = t1["geladen"].as_u64().unwrap() + t2["geladen"].as_u64().unwrap();
        assert!(geladen > 0, "Zellen wurden geladen");
        let resident = reg.call("world.cells.resident", json!({})).unwrap();
        assert!(resident["anzahl"].as_u64().unwrap() > 0);
        // Zustand einer geladenen Zelle abfragen.
        let z = &resident["zellen"][0];
        let state = reg
            .call(
                "world.cell.state",
                json!({ "ebene": z["ebene"], "x": z["x"], "z": z["z"] }),
            )
            .unwrap();
        assert_eq!(state["zustand"], json!("ready"));
        // Unbekannte Zelle.
        let state = reg
            .call("world.cell.state", json!({ "ebene": 0, "x": 99999, "z": 99999 }))
            .unwrap();
        assert_eq!(state["zustand"], json!("none"));
    }

    #[test]
    fn fehlerpfade_sind_sauber() {
        let reg = rig();
        // Fehlender Parameter.
        assert!(reg.call("world.camera.set", json!({ "x": 1.0 })).is_err());
        // Nan/unsinnige dt wird abgewiesen.
        assert!(reg.call("world.tick", json!({ "dt": -1.0 })).is_err());
        // Unbekanntes Werkzeug.
        assert!(reg.call("world.es_gibt_mich_nicht", json!({})).is_err());
    }

    #[test]
    fn stats_zaehlen_mit() {
        let reg = rig();
        reg.call("world.instance.spawn", json!({ "x": 0.0, "z": 0.0 })).unwrap();
        reg.call("world.tick", json!({ "dt": 0.016 })).unwrap();
        let stats = reg.call("world.stats", json!({})).unwrap();
        assert_eq!(stats["instanzen"], json!(1));
        assert!(stats["clipmap"]["aktive_zellen"].as_u64().unwrap() > 0);
        assert_eq!(stats["io_telemetrie"]["fehler"], json!(0));
        assert_eq!(stats["hlod"]["cluster"], json!(0), "ohne Bake keine Cluster");
        assert_eq!(reg.call("world.hlod.list", json!({})).unwrap()["anzahl"], json!(0));
    }
}
