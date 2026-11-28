//! Wavefront OBJ reader/writer with MTL materials — der Legacy-Pfad
//! neben glTF.
//!
//! Import: `v`/`vt`/`vn`, Polygone werden gefächert, negative Indizes
//! (relativ) unterstützt. Gruppen (`g`/`o`) × verwandte Materialien
//! ergeben je ein Mesh mit eigenem Szenenknoten; `usemtl` bindet das
//! Material aus dem `mtllib`-File (`newmtl`, `Kd`, `Ns`, `d`/`Tr`,
//! `map_Kd`, `norm`). Fehlende Normalen werden als Flächenormalen
//! ergänzt; OBJ-`vt` (Unten-links-Ursprung) werden auf die
//! Engine-Konvention (oben links) gespiegelt.
//!
//! Export: die Maschen des Projekts werden mit angelegter
//! Knotentransformation als `.obj` + `.mtl` geschrieben — keine
//! Hierarchie, dafür lesen alle DCC-Tools das Ergebnis.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use re_core::id::MaterialId;
use re_core::material::Material;
use re_core::mesh::Mesh;
use re_core::project::Project;
use re_core::scene::{Node, NodeKind};
use re_core::transform::Transform;

// ── MTL ──────────────────────────────────────────────────────────────

/// Ein aus `*.mtl` gelesener Materialeintrag.
#[derive(Clone, Debug)]
struct MtlEntry {
    kd: [f32; 4],
    /// Rohwert 0..100 (Glanz) — wird zu `roughness = 1 − Ns/100`.
    ns: f32,
    albedo: Option<String>,
    normal: Option<String>,
}

fn parse_mtl(text: &str) -> HashMap<String, MtlEntry> {
    let mut out = HashMap::new();
    let mut cur: Option<(String, MtlEntry)> = None;
    let blank = || MtlEntry { kd: [0.8, 0.8, 0.8, 1.0], ns: 0.0, albedo: None, normal: None };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let val = line[key.len()..].trim();
        if key == "newmtl" {
            if let Some((name, e)) = cur.take() {
                out.insert(name, e);
            }
            cur = Some((val.to_string(), blank()));
            continue;
        }
        let Some((_, e)) = cur.as_mut() else { continue };
        match key {
            "Kd" => {
                let n: Vec<f32> = val.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                e.kd = [
                    n.first().copied().unwrap_or(0.8),
                    n.get(1).copied().unwrap_or(0.8),
                    n.get(2).copied().unwrap_or(0.8),
                    n.get(3).copied().unwrap_or(1.0),
                ];
            }
            "Ns" => e.ns = val.parse().unwrap_or(0.0),
            "d" => e.kd[3] = val.parse().unwrap_or(1.0),
            "Tr" => e.kd[3] = (1.0 - val.parse::<f32>().unwrap_or(0.0)).clamp(0.0, 1.0),
            "map_Kd" => e.albedo = Some(val.to_string()),
            "map_Bump" | "bump" | "norm" => e.normal = Some(val.to_string()),
            _ => {}
        }
    }
    if let Some((name, e)) = cur.take() {
        out.insert(name, e);
    }
    out
}

fn mtl_to_material(name: &str, e: &MtlEntry) -> Material {
    Material {
        name: name.to_string(),
        base_color: e.kd,
        metallic: 0.0,
        roughness: (1.0 - (e.ns / 100.0).clamp(0.0, 1.0)).clamp(0.0, 1.0),
        emissive: [0.0, 0.0, 0.0],
        double_sided: false,
        albedo_texture_id: e.albedo.clone(),
        normal_texture_id: e.normal.clone(),
    }
}

// ── Import ───────────────────────────────────────────────────────────

/// Ein Flächeneck: OBJ-Indizes (1-basiert; negativ = relativ auf den
/// bisherigen Listenstand), 0 = Feld fehlte.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Corner {
    v: i64,
    t: i64,
    n: i64,
}

#[derive(Default)]
struct ObjFile {
    pos: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    nrm: Vec<[f32; 3]>,
    /// (Gruppenname, Materialname) → Dreiecksecken.
    parts: Vec<(String, Option<String>, Vec<Corner>)>,
    cur_group: Option<String>,
    cur_mtl: Option<String>,
}

/// Ein `f`-Token („12/3/5", „12//5", „12", auch negativ) in einen
/// [`Corner`] übersetzen; negative Zahlen lösen gegen die aktuelle
/// Listenlänge auf.
fn parse_face_tok(tok: &str, f: &ObjFile) -> Corner {
    let mut c = Corner { v: 0, t: 0, n: 0 };
    for (slot, s, len) in [
        (&mut c.v as &mut i64, 0usize, f.pos.len()),
        (&mut c.t as &mut i64, 1usize, f.uv.len()),
        (&mut c.n as &mut i64, 2usize, f.nrm.len()),
    ] {
        if let Some(field) = tok.split('/').nth(s).filter(|x| !x.is_empty()) {
            if let Ok(x) = field.parse::<i64>() {
                *slot = if x < 0 { x + len as i64 + 1 } else { x };
            }
        }
    }
    c
}

/// Ziel-Teil für (Gruppe, Material) holen oder neu anlegen.
fn part_slot<'a>(f: &'a mut ObjFile, group: &str, mtl: &Option<String>) -> &'a mut Vec<Corner> {
    let at = f.parts.iter().position(|(g, m, _)| g.as_str() == group && m == mtl);
    match at {
        Some(i) => &mut f.parts[i].2,
        None => {
            f.parts.push((group.to_string(), mtl.clone(), Vec::new()));
            &mut f.parts.last_mut().unwrap().2
        }
    }
}

fn import_obj(text: &str) -> Result<ObjFile> {
    let mut f = ObjFile::default();
    // Zeilenfortsetzung mit "\" vor dem Umbruch auflösen.
    let joined = text.replace("\\\n", " ");
    for (ln, raw) in joined.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let err = |m: String| anyhow::anyhow!("{m} (Zeile {}: {line})", ln + 1);
        match key {
            "v" | "vn" | "vt" => {
                let nums: Vec<f32> = it.filter_map(|t| t.parse().ok()).collect();
                match key {
                    "v" if nums.len() >= 3 => f.pos.push([nums[0], nums[1], nums[2]]),
                    "vn" if nums.len() >= 3 => f.nrm.push([nums[0], nums[1], nums[2]]),
                    // vt y-Achse: OBJ unten-links → Engine oben-links.
                    "vt" if !nums.is_empty() => {
                        let u = nums[0];
                        let v = nums.get(1).copied().unwrap_or(0.0);
                        f.uv.push([u, 1.0 - v]);
                    }
                    _ => {}
                }
            }
            "f" => {
                let toks: Vec<&str> = it.collect();
                if toks.len() < 3 {
                    return Err(err(format!("Polygon mit {} Ecken", toks.len())));
                }
                let corners: Vec<Corner> = toks.iter().map(|t| parse_face_tok(t, &f)).collect();
                for c in &corners {
                    if c.v < 1 || c.v as usize > f.pos.len() {
                        return Err(err(format!("v-Index {} außerhalb", c.v)));
                    }
                }
                // Polygon fächern: (0, i, i+1).
                let gname = f.cur_group.clone().unwrap_or_default();
                let mname = f.cur_mtl.clone();
                let slot = part_slot(&mut f, &gname, &mname);
                for i in 1..corners.len() - 1 {
                    slot.push(corners[0]);
                    slot.push(corners[i]);
                    slot.push(corners[i + 1]);
                }
            }
            "g" | "o" => f.cur_group = it.next().map(str::to_string),
            "usemtl" => f.cur_mtl = it.next().map(str::to_string),
            _ => {} // mtllib wird vorab gescannt; unbekanntes ignorieren
        }
    }
    if f.parts.iter().all(|(_, _, c)| c.is_empty()) {
        bail!("OBJ enthält keine Flächen");
    }
    Ok(f)
}

/// OBJ-Datei zu einem Projekt (ein Knoten je Gruppe × Material).
pub fn import(path: &Path) -> Result<Project> {
    let text = fs::read_to_string(path).with_context(|| format!("open {}", path.display()))?;
    let obj = import_obj(&text)?;

    // Materialien: alle mtllib-Dateien neben dem OBJ einsammeln.
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut mtl_db: HashMap<String, MtlEntry> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("mtllib") {
            for name in rest.split_whitespace() {
                let p: PathBuf = dir.join(name);
                if let Ok(t) = fs::read_to_string(&p) {
                    for (k, v) in parse_mtl(&t) {
                        mtl_db.insert(k, v);
                    }
                }
            }
        }
    }
    let mut mat_cache: HashMap<String, MaterialId> = HashMap::new();

    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("imported");
    let mut p = Project::new(stem);
    for (group, mtl_name, corners) in obj.parts {
        if corners.is_empty() {
            continue;
        }
        // Ecken deduplizieren und in kompakte Vertex-Listen überführen.
        let mut map: HashMap<Corner, u32> = HashMap::new();
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut normals: Vec<[f32; 3]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut any_uv = false;
        let mut any_nrm = false;
        for c in &corners {
            let idx = *map.entry(*c).or_insert_with(|| {
                let i = positions.len() as u32;
                positions.push(obj.pos[c.v as usize - 1]);
                if c.t > 0 && (c.t as usize) <= obj.uv.len() {
                    any_uv = true;
                    uvs.push(obj.uv[c.t as usize - 1]);
                } else {
                    uvs.push([0.0, 0.0]);
                }
                if c.n > 0 && (c.n as usize) <= obj.nrm.len() {
                    any_nrm = true;
                    normals.push(obj.nrm[c.n as usize - 1]);
                } else {
                    normals.push([0.0, 0.0, 0.0]);
                }
                i
            });
            indices.push(idx);
        }
        if !any_nrm {
            // Flächenormalen ergänzen — OBJ importiert ungeglättet, das
            // ist für Import-Ansichten die ehrliche Interpretation.
            for tri in indices.chunks(3) {
                let (a, b, cc) = (
                    positions[tri[0] as usize],
                    positions[tri[1] as usize],
                    positions[tri[2] as usize],
                );
                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let ac = [cc[0] - a[0], cc[1] - a[1], cc[2] - a[2]];
                let n = [
                    ab[1] * ac[2] - ab[2] * ac[1],
                    ab[2] * ac[0] - ab[0] * ac[2],
                    ab[0] * ac[1] - ab[1] * ac[0],
                ];
                for &i in tri {
                    normals[i as usize] = n;
                }
            }
        }
        let mut mesh = Mesh {
            name: if group.is_empty() { format!("{stem}_mesh") } else { group.clone() },
            positions,
            normals: Some(normals),
            uvs: if any_uv { Some(uvs) } else { None },
            indices: Some(indices),
            skin: None,
            stamp: Default::default(),
        };
        mesh.touch();
        let mid = p.library.add_mesh(mesh);
        let material = mtl_name.as_ref().map(|name| {
            if let Some(id) = mat_cache.get(name) {
                return *id;
            }
            let mat = match mtl_db.get(name) {
                Some(e) => mtl_to_material(name, e),
                None => Material { name: name.clone(), ..Default::default() },
            };
            let id = p.library.add_material(mat);
            mat_cache.insert(name.clone(), id);
            id
        });
        let node = Node {
            name: if group.is_empty() { stem.to_string() } else { group.clone() },
            transform: Transform::default(),
            kind: NodeKind::MeshInstance { mesh: mid, material, skeleton_node: None },
            ..Default::default()
        };
        p.scene.insert(None, node);
    }
    Ok(p)
}

// ── Export ───────────────────────────────────────────────────────────

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_whitespace() || c == '/' { '_' } else { c })
        .collect();
    if s.is_empty() { "obj".to_string() } else { s }
}

/// Projekt als `.obj` (+ `.mtl`, daneben geschrieben) exportieren.
/// Knotentransformationen werden in die Vertex-Positionen gebacken —
/// OBJ kennt keine Hierarchie.
pub fn export(p: &Project, path: &Path) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    if !dir.as_os_str().is_empty() {
        fs::create_dir_all(dir).with_context(|| format!("create dir {}", dir.display()))?;
    }
    let stem = sanitize(path.file_stem().and_then(|s| s.to_str()).unwrap_or("export"));
    let mtl_name = format!("{stem}.mtl");

    let mut out = String::new();
    out.push_str("# geschrieben von re-io\n");
    let mut mtls = String::new();
    let mut used_mtls: Vec<MaterialId> = Vec::new();
    // OBJ-Indizes sind dateiweit gültig — die Schreiber zählen mit.
    let (mut n_v, mut n_t, mut n_n) = (0usize, 0usize, 0usize);

    for node in p.scene.nodes.values() {
        let NodeKind::MeshInstance { mesh, material, .. } = &node.kind else { continue };
        let Some(m) = p.library.meshes.get(mesh) else { continue };
        let name = sanitize(&m.name);
        out.push_str(&format!("o {name}\n"));
        // usemtl gilt für alle *folgenden* Flächen — also vorab schreiben.
        if let Some(id) = material {
            if let Some(mat) = p.library.materials.get(id) {
                out.push_str(&format!("usemtl {}\n", sanitize(&mat.name)));
            }
            if !used_mtls.contains(id) {
                used_mtls.push(*id);
            }
        }
        let v_base = n_v;
        let t_base = n_t;
        let nn_base = n_n;
        n_v += m.positions.len();
        if m.uvs.is_some() {
            n_t += m.uvs.as_ref().map_or(0, |v| v.len());
        }
        if m.normals.is_some() {
            n_n += m.normals.as_ref().map_or(0, |v| v.len());
        }
        // Positionen transformieren, Normalen über die inverse
        // Transponierte (Skalierungen erhalten Richtungen).
        let tr = node.transform.to_mat4();
        let nmat = tr.inverse().transpose();
        for pt in &m.positions {
            let v = tr.transform_point3(glam::Vec3::from(*pt));
            out.push_str(&format!("v {} {} {}\n", v.x, v.y, v.z));
        }
        for t in m.uvs.iter().flatten() {
            // zurück auf OBJ-Konvention (unten links).
            out.push_str(&format!("vt {} {}\n", t[0], 1.0 - t[1]));
        }
        for n in m.normals.iter().flatten() {
            let v = nmat.transform_vector3(glam::Vec3::from(*n)).normalize_or_zero();
            out.push_str(&format!("vn {} {} {}\n", v.x, v.y, v.z));
        }
        let (has_t, has_n) = (m.uvs.is_some(), m.normals.is_some());
        let corners: Vec<u32> = m
            .indices
            .clone()
            .unwrap_or_else(|| (0..m.positions.len() as u32).collect());
        let fmt_idx = |i: u32| -> String {
            let i = i as usize;
            match (has_t, has_n) {
                (true, true) => format!("{}/{}/{}", v_base + i + 1, t_base + i + 1, nn_base + i + 1),
                (true, false) => format!("{}/{}", v_base + i + 1, t_base + i + 1),
                (false, true) => format!("{}//{}", v_base + i + 1, nn_base + i + 1),
                (false, false) => format!("{}", v_base + i + 1),
            }
        };
        for tri in corners.chunks(3) {
            if tri.len() != 3 {
                bail!("{}: Dreiecksliste nicht durch 3 teilbar", m.name);
            }
            out.push_str(&format!("f {} {} {}\n", fmt_idx(tri[0]), fmt_idx(tri[1]), fmt_idx(tri[2])));
        }
    }

    // MTL-Blöcke für alle verwendeten Materialien.
    for id in &used_mtls {
        let Some(mat) = p.library.materials.get(id) else { continue };
        let nm = sanitize(&mat.name);
        let [r, g, b, a] = mat.base_color;
        mtls.push_str(&format!("newmtl {nm}\nKd {r} {g} {b} {a}\nNs {}\n", (1.0 - mat.roughness) * 100.0));
        if let Some(t) = &mat.albedo_texture_id {
            mtls.push_str(&format!("map_Kd {t}\n"));
        }
        if let Some(t) = &mat.normal_texture_id {
            mtls.push_str(&format!("norm {t}\n"));
        }
        mtls.push('\n');
    }
    if !used_mtls.is_empty() {
        // mtllib direkt nach dem Kopf-Kommentar einfügen.
        let at = out.find('\n').map(|i| i + 1).unwrap_or(0);
        out.insert_str(at, &format!("mtllib {mtl_name}\n"));
        fs::write(dir.join(&mtl_name), &mtls).with_context(|| format!("mtl {mtl_name}"))?;
    }
    fs::write(path, out).with_context(|| format!("obj {}", path.display()))?;
    Ok(())
}
