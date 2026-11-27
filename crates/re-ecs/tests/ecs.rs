//! Verhalten der Welt: Handles, Komponenten-Speicher, Join und Schedule.

use re_ecs::{Entity, Schedule, World};

#[derive(Debug, Clone, PartialEq)]
struct Health(u32);

#[derive(Debug, Clone, PartialEq)]
struct Position {
    x: f32,
    y: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct Velocity {
    dx: f32,
    dy: f32,
}

#[test]
fn spawn_und_despawn_zaehlen() {
    let mut world = World::new();
    assert!(world.is_empty());

    let a = world.spawn();
    let b = world.spawn();
    assert_ne!(a, b);
    assert_eq!(world.len(), 2);
    assert!(world.alive(a));

    assert!(world.despawn(b));
    assert!(!world.alive(b));
    assert_eq!(world.len(), 1);
    assert_eq!(world.entities(), vec![a]);

    // Zweites Entfernen desselben Handles: abgewiesen.
    assert!(!world.despawn(b));
}

#[test]
fn veraltetes_handle_ist_tot_aber_slot_lebt_weiter() {
    let mut world = World::new();
    let a = world.spawn();
    world.insert(a, Health(10));
    world.despawn(a);

    // Neuer Bewohner desselben Slots bekommt höhere Generation.
    let c = world.spawn();
    assert_eq!(c.index(), a.index());
    assert_eq!(c.generation(), a.generation() + 1);
    assert!(world.alive(c));
    assert!(!world.alive(a));

    // Alte Handles schreiben und lesen nichts.
    assert_eq!(world.insert(a, Health(1)), None);
    assert!(world.get::<Health>(a).is_none());
    assert!(world.get::<Health>(c).is_none());
}

#[test]
fn einfügen_ersetzt_und_entfernen_gibt_wert_zurueck() {
    let mut world = World::new();
    let e = world.spawn();

    assert_eq!(world.insert(e, Health(3)), None);
    assert_eq!(world.insert(e, Health(7)), Some(Health(3)));
    assert_eq!(world.get(e), Some(&Health(7)));
    assert!(world.has::<Health>(e));
    assert_eq!(world.count::<Health>(), 1);

    if let Some(h) = world.get_mut::<Health>(e) {
        h.0 = 9;
    }
    assert_eq!(world.get(e), Some(&Health(9)));

    assert_eq!(world.remove::<Health>(e), Some(Health(9)));
    assert!(!world.has::<Health>(e));
    assert_eq!(world.count::<Health>(), 0);
    assert_eq!(world.remove::<Health>(e), None);
}

#[test]
fn despawn_raeumt_alle_komponenten() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Health(1));
    world.insert(e, Position { x: 0.0, y: 0.0 });
    world.despawn(e);

    assert_eq!(world.count::<Health>(), 0);
    assert_eq!(world.count::<Position>(), 0);
}

#[test]
fn iteration_ueber_eine_und_zwei_spalten() {
    let mut world = World::new();
    for i in 0..5 {
        let e = world.spawn();
        world.insert(e, Position { x: i as f32, y: 0.0 });
        if i % 2 == 0 {
            world.insert(e, Velocity { dx: 1.0, dy: 0.0 });
        }
    }

    world.for_each_mut(|_: Entity, p: &mut Position| p.x += 100.0);

    let mut joined = 0;
    world.for_each_pair(|_, p: &mut Position, v: &mut Velocity| {
        p.x += v.dx;
        joined += 1;
    });
    assert_eq!(joined, 3);

    // Gerades i (0, 2, 4) trug zusätzlich Velocity und bekam +1.
    let alle: Vec<f32> = {
        let mut v = Vec::new();
        world.for_each(|_, p: &Position| v.push(p.x));
        v.sort_by(|a, b| a.total_cmp(b));
        v
    };
    assert_eq!(alle, vec![101.0, 101.0, 103.0, 103.0, 105.0]);
}

#[test]
fn schedule_laeuft_in_einfuegereihenfolge_mit_dt() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Position { x: 0.0, y: 0.0 });
    world.insert(
        e,
        Velocity {
            dx: 2.0,
            dy: -0.5,
        },
    );

    let log = Rc::new(RefCell::new(Vec::new()));
    let mut schedule = Schedule::new();
    {
        let log = log.clone();
        schedule.add(move |world: &mut World, dt: f32| {
            log.borrow_mut().push("bewegung");
            world.for_each_pair(|_, p: &mut Position, v: &mut Velocity| {
                p.x += v.dx * dt;
                p.y += v.dy * dt;
            });
        });
    }
    {
        let log = log.clone();
        schedule.add(move |_: &mut World, _| log.borrow_mut().push("nachlesen"));
    }

    assert_eq!(schedule.len(), 2);
    schedule.run(&mut world, 0.5);
    schedule.run(&mut world, 0.5);

    assert_eq!(schedule.tick(), 2);
    let p = world.get::<Position>(e).unwrap();
    assert_eq!((p.x, p.y), (2.0, -0.5));
    assert_eq!(
        *log.borrow(),
        vec!["bewegung", "nachlesen", "bewegung", "nachlesen"]
    );
}
