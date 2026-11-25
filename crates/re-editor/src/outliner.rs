//! Outliner & Inspector über dem ECS (M4b).
//!
//! Der Outliner listet Entities und ihre Components; der Inspector liest
//! und schreibt Component-Felder als JSON. Weil `re-ecs` generisch und
//! typgelöscht arbeitet, führt der [`ComponentRegistry`] die Typen, die
//! inspizierbar sein sollen (mit Name, Serialisierung und Schreiben).

use std::any::TypeId;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use re_agent::capability::{CapabilityRegistry, ToolSpec};
use re_ecs::World;
use re_ecs::Entity;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Editor-spezifische Component: Anzeigename einer Entity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Name {
    pub value: String,
}

impl Name {
    pub fn new(value: impl Into<String>) -> Self { Self { value: value.into() } }
}

/// Editor-spezifische Component: Sichtbarkeit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Visible {
    pub value: bool,
}

impl Default for Visible {
    fn default() -> Self { Self { value: true } }
}

/// Typgelöschter Zugriff auf eine Component.
struct ComponentEntry {
    type_id: TypeId,
    type_name: &'static str,
    list_entities: Box<dyn Fn(&World) -> Vec<Entity> + Send + Sync>,
    to_json: Box<dyn Fn(&World, Entity) -> Option<Value> + Send + Sync>,
    from_json: Box<dyn Fn(&mut World, Entity, Value) -> Result<(), String> + Send + Sync>,
}

/// Registry der inspizierbaren Components. Wird einmal aufgebaut und
/// mit dem ECS-World geteilt.
pub struct ComponentRegistry {
    entries: BTreeMap<String, ComponentEntry>,
}

impl ComponentRegistry {
    pub fn new() -> Self { Self { entries: BTreeMap::new() } }

    pub fn register<T>(&mut self, name: &'static str)
    where
        T: re_ecs::Component + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();
        self.entries.insert(name.to_string(), ComponentEntry {
            type_id,
            type_name: name,
            list_entities: Box::new(|w: &World| w.entities().into_iter().filter(|e| w.has::<T>(*e)).collect()),
            to_json: Box::new(|w: &World, e: Entity| w.get::<T>(e).map(|c| serde_json::to_value(c).unwrap_or(Value::Null))),
            from_json: Box::new(|w: &mut World, e: Entity, v: Value| {
                let c: T = serde_json::from_value(v).map_err(|e| e.to_string())?;
                w.insert(e, c);
                Ok(())
            }),
        });
    }

    pub fn type_names(&self) -> Vec<&'static str> {
        self.entries.values().map(|e| e.type_name).collect()
    }

    pub fn has_type(&self, name: &str) -> bool { self.entries.contains_key(name) }

    pub fn entities_with(&self, world: &World, name: &str) -> Option<Vec<Entity>> {
        self.entries.get(name).map(|e| (e.list_entities)(world))
    }

    pub fn to_json(&self, world: &World, name: &str, entity: Entity) -> Option<Value> {
        self.entries.get(name).and_then(|e| (e.to_json)(world, entity))
    }

    pub fn from_json(&self, world: &mut World, name: &str, entity: Entity, value: Value) -> Result<(), String> {
        match self.entries.get(name) {
            Some(e) => (e.from_json)(world, entity, value),
            None => Err(format!("unbekannte Component: {name}")),
        }
    }

    pub fn type_of(&self, type_id: TypeId) -> Option<&'static str> {
        self.entries.values().find(|e| e.type_id == type_id).map(|e| e.type_name)
    }
}

impl Default for ComponentRegistry {
    fn default() -> Self { Self::new() }
}

// ── Shared handles ────────────────────────────────────────────────────
pub type SharedWorld = Arc<Mutex<World>>;
pub type SharedRegistry = Arc<ComponentRegistry>;

fn lock_world(w: &SharedWorld) -> Result<MutexGuard<'_, World>, String> {
    w.lock().map_err(|_| "ECS-Sperre vergiftet".to_string())
}

// ── Outliner ──────────────────────────────────────────────────────────
pub struct Outliner {
    pub world: SharedWorld,
    pub registry: SharedRegistry,
}

impl Outliner {
    pub fn new(world: SharedWorld, registry: SharedRegistry) -> Self {
        Self { world, registry }
    }

    pub fn entity_count(&self) -> usize {
        self.world.lock().map(|w| w.len()).unwrap_or(0)
    }

    pub fn entities(&self) -> Vec<Entity> {
        self.world.lock().map(|w| w.entities()).unwrap_or_default()
    }

    pub fn components_of(&self, entity: Entity) -> Vec<&'static str> {
        let reg = &*self.registry;
        let w = match self.world.lock() { Ok(g) => g, Err(_) => return Vec::new() };
        reg.type_names().into_iter().filter(|n| {
            reg.entities_with(&w, n).map(|v| v.contains(&entity)).unwrap_or(false)
        }).collect()
    }
}

// ── Capabilities ──────────────────────────────────────────────────────
pub fn register_outliner_tools(
    reg: &mut CapabilityRegistry,
    world: SharedWorld,
    registry: SharedRegistry,
) {
    {
        let w = world.clone();
        reg.register(
            ToolSpec::simple("ecs.entities", "Alle Entities auflisten"),
            move |_| {
                let guard = lock_world(&w)?;
                let entities: Vec<u32> = guard.entities().iter().map(|e| e.index()).collect();
                Ok(json!({ "entities": entities, "anzahl": entities.len() }))
            },
        );
    }
    {
        let w = world.clone();
        let r = registry.clone();
        reg.register(
            ToolSpec::new("ecs.entity.components", "Component-Typen einer Entity", json!({ "typ": "object", "felder": { "entity": { "typ": "integer" } } })),
            move |args| {
                let id = args.get("entity").and_then(|v| v.as_u64()).ok_or("Parameter 'entity' fehlt")? as u32;
                let guard = lock_world(&w)?;
                // Entity über den Index suchen (Generation ist dem JSON nicht zu entnehmen).
                let ent = guard.entities().into_iter().find(|e| e.index() == id).ok_or("Entity nicht gefunden")?;
                let comps = r.type_names().into_iter().filter(|n| {
                    r.entities_with(&guard, n).map(|v| v.contains(&ent)).unwrap_or(false)
                }).collect::<Vec<_>>();
                Ok(json!({ "entity": id, "components": comps }))
            },
        );
    }
    {
        let w = world.clone();
        let r = registry.clone();
        reg.register(
            ToolSpec::new("ecs.entity.get", "Component einer Entity als JSON", json!({ "typ": "object", "felder": { "entity": { "typ": "integer" }, "component": { "typ": "string" } } })),
            move |args| {
                let id = args.get("entity").and_then(|v| v.as_u64()).ok_or("Parameter 'entity' fehlt")? as u32;
                let comp = args.get("component").and_then(|v| v.as_str()).ok_or("Parameter 'component' fehlt")?;
                let guard = lock_world(&w)?;
                let ent = guard.entities().into_iter().find(|e| e.index() == id).ok_or("Entity nicht gefunden")?;
                match r.to_json(&guard, comp, ent) {
                    Some(v) => Ok(json!({ "entity": id, "component": comp, "wert": v })),
                    None => Err(format!("keine Component '{comp}' an Entity {id}")),
                }
            },
        );
    }
    {
        let w = world.clone();
        let r = registry.clone();
        reg.register(
            ToolSpec::new("ecs.entity.set", "Component-Felder einer Entity setzen", json!({ "typ": "object", "felder": { "entity": { "typ": "integer" }, "component": { "typ": "string" }, "wert": { "typ": "object" } } })),
            move |args| {
                let id = args.get("entity").and_then(|v| v.as_u64()).ok_or("Parameter 'entity' fehlt")? as u32;
                let comp = args.get("component").and_then(|v| v.as_str()).ok_or("Parameter 'component' fehlt")?;
                let val = args.get("wert").cloned().ok_or("Parameter 'wert' fehlt")?;
                let mut guard = lock_world(&w)?;
                let ent = guard.entities().into_iter().find(|e| e.index() == id).ok_or("Entity nicht gefunden")?;
                r.from_json(&mut guard, comp, ent, val).map_err(|e| e)?;
                Ok(json!({ "ok": true }))
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use re_agent::CapabilityRegistry;
    use re_core::transform::Transform;
    use serde_json::json;

    fn world_with_entities() -> (SharedWorld, SharedRegistry) {
        let mut w = World::new();
        let e1 = w.spawn();
        w.insert(e1, Transform::default());
        w.insert(e1, Name::new("Player"));
        let e2 = w.spawn();
        w.insert(e2, Transform { translation: [1.0, 2.0, 3.0], ..Default::default() });
        let mut reg = ComponentRegistry::new();
        reg.register::<Transform>("Transform");
        reg.register::<Name>("Name");
        (Arc::new(Mutex::new(w)), Arc::new(reg))
    }

    #[test]
    fn entities_und_components() {
        let (w, r) = world_with_entities();
        let mut reg = CapabilityRegistry::new();
        register_outliner_tools(&mut reg, w.clone(), r.clone());
        let list = reg.call("ecs.entities", json!({})).unwrap();
        assert_eq!(list["anzahl"], json!(2));
        let comps = reg.call("ecs.entity.components", json!({ "entity": 0 })).unwrap();
        assert!(comps["components"].as_array().unwrap().contains(&json!("Transform")));
        assert!(comps["components"].as_array().unwrap().contains(&json!("Name")));
    }

    #[test]
    fn get_und_set_component() {
        let (w, r) = world_with_entities();
        let mut reg = CapabilityRegistry::new();
        register_outliner_tools(&mut reg, w.clone(), r.clone());
        let got = reg.call("ecs.entity.get", json!({ "entity": 1, "component": "Transform" })).unwrap();
        assert_eq!(got["wert"]["translation"], json!([1.0, 2.0, 3.0]));
        reg.call("ecs.entity.set", json!({ "entity": 1, "component": "Transform", "wert": { "translation": [9.0, 9.0, 9.0], "rotation": [0.0,0.0,0.0,1.0], "scale": [1.0,1.0,1.0] } })).unwrap();
        let got = reg.call("ecs.entity.get", json!({ "entity": 1, "component": "Transform" })).unwrap();
        assert_eq!(got["wert"]["translation"], json!([9.0, 9.0, 9.0]));
    }

    #[test]
    fn unbekannte_component_fehler() {
        let (w, r) = world_with_entities();
        let mut reg = CapabilityRegistry::new();
        register_outliner_tools(&mut reg, w.clone(), r.clone());
        assert!(reg.call("ecs.entity.get", json!({ "entity": 0, "component": "Foo" })).is_err());
        assert!(reg.call("ecs.entity.set", json!({ "entity": 0, "component": "Foo", "wert": {} })).is_err());
    }
}
