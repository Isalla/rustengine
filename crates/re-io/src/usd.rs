//! Reader für die ASCII-Teilmenge von USD (OpenUSD-Wurzeldateien
//! `.usd`/`.usda`; Import nur — Export bleibt glTF/OBJ).
//!
//! Verstanden wird das, was Editoren und Exporter (Blender USD IO, Maya,
//! Houdini) als Szenen-Austausch schreiben, ohne Composability-Mechanik:
//!
//! - Prims `def`/`over`/`class` mit Typ, Namen und verschachtelten Blöcken;
//!   unbekannte Typen werden zu leeren Szenenknoten (Container bleiben erhalten).
//! - Transform-Operations: `uniform token[] xformOpOrder` (oder das ältere
//!   `xformOps`) — die Liste läuft von der äußeren zur innersten Operation,
//!   angewandt wird gegen die Listenreihenfolge (der letzte Eintrag wirkt
//!   zuerst; das übliche `[translate, rotate, scale]` heißt also
//!   „erst skaliert, dann rotiert, dann translatiert“). Verstanden:
//!   `xformOp:translate`, `:scale` (auch X/Y/Z einzeln), `:rotateX/Y/Z`,
//!   `:rotateABC` (die drei Achsdrehungen werden in genannter Reihenfolge
//!   um feste Weltachsen konsumiert, Grad), `:orient` (Quaternion, w
//!   zuerst) und `xformOp`/`xformOp:transform` als `matrix4d` (Gf-speichert
//!   zeilenmajor mit Übersetzung in der letzten Zeile). `!invert!`-Präfix
//!   invertiert die Operation; `!resetXformStack!` wird ignoriert, da der
//!   Import ohnehin bei der Identität startet.
//! - `def Mesh`: `point3f[] points`, `int[] faceVertexIndices`,
//!   `int[] faceVertexCounts` (Polygone als Fächer trianguliert),
//!   `uniform token orientation` („leftHanded“ kehrt die
//!   Dreiecksreihenfolge um — das Engine-Front-Face ist CCW),
//!   `normal3f[] normals` und `point2f[] primvars:st` (bei passender Texelzahl).
//! - `def Material` mit `bool doubleSided` und untergeordneten
//!   `def Shader`-Prims vom Typ `UsdPreviewSurface`
//!   (`inputs:diffuseColor`, `inputs:opacity`, `inputs:metallic`,
//!   `inputs:roughness`, `inputs:emissionColor`) sowie `UsdUVTexture`
//!   (`asset file`) als Albedo-Textur.
//! - Material-Bindungen: `rel material:binding = </Pfad/Mat>` (auch als
//!   `( material:binding = … )`-Metadatum) wirken auf den Prim und alle
//!   Nachkommen, wie in UsdShade vorgesehen.
//! - Stage-Metadaten `upAxis`: Dateien mit `upAxis = "Z"` bekommen
//!   denselben −90°-X-Korrekturknoten über allen Wurzeln wie beim
//!   FBX-Importer.
//!
//! Bewusst außerhalb der Teilmenge (mit klarem Fehler statt stiller Lücke):
//! externe Referenzen und Payloads (`references = @…@`), Varianten
//! (`variantSet`/`variantSets`/`variants`), Referenz-Transforms
//! (`xformOp:transform` auf Namespace-Prim), Time-Samples (es zählt der
//! authored Default-Wert) sowie binäre `.usdc`-Dateien. Kameras und
//! Skelette werden wie bei glTF/FBX nicht als Sonderknoten importiert —
//! sie bleiben leere Knoten in der Hierarchie.

use anyhow::{Context, Result, bail};
use glam::{Mat4, Quat, Vec3};
use std::collections::HashMap;
use std::path::Path;

use re_core::id::{MaterialId, NodeId};
use re_core::material::Material;
use re_core::mesh::Mesh as WMesh;
use re_core::project::Project;
use re_core::scene::{Node as WNode, NodeKind};
use re_core::transform::Transform;

// ───────────────────────────── Tokenizer ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    /// Bezeichner, Zahl, Pfad `</a/b>`, Asset `@datei.ext@` — als Text.
    Word(String),
    /// quoted string mit aufgelösten Escapes.
    Str(String),
    Punct(char),
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '/' | '<' | '>' | '@' | '\\')
}

fn tokenize(src: &str) -> Result<Vec<Tok>> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '#' {
            // Kommentar bis Zeilenende (`#usda 1.0`-Kopf inklusive).
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '"' {
            let mut s = String::new();
            i += 1;
            loop {
                if i >= b.len() {
                    bail!("USD: Zeilenende innerhalb eines Strings");
                }
                match b[i] {
                    '"' => {
                        i += 1;
                        break;
                    }
                    '\\' if i + 1 < b.len() => {
                        i += 1;
                        s.push(match b[i] {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                        i += 1;
                    }
                    other => {
                        s.push(other);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Str(s));
            continue;
        }
        if matches!(c, '{' | '}' | '[' | ']' | '(' | ')' | '=' | ',' | ';') {
            out.push(Tok::Punct(c));
            i += 1;
            continue;
        }
        let negative = c == '-' && i + 1 < b.len() && (b[i + 1].is_ascii_digit() || b[i + 1] == '.');
        if !negative && !is_word_char(c) {
            bail!("USD: unerwartetes Zeichen {c:?}");
        }
        let start = i;
        if negative {
            i += 1;
        }
        while i < b.len() && is_word_char(b[i]) {
            i += 1;
        }
        out.push(Tok::Word(b[start..i].iter().collect()));
    }
    Ok(out)
}

// ─────────────────────────── Syntaxbaum ──────────────────────────────

#[derive(Clone, Debug)]
enum Val {
    Atom(String),
    /// Sequenz aus `[…]`-Arrays und `(…)`-Tupeln.
    Seq(Vec<Val>),
    /// Übersprangener `{…}`-Block (Dictionary-Metadaten).
    Opaque,
}

impl Val {
    /// Alle Atom-Werte, rekursiv entpackt (für Zahlenvektoren und Tokens).
    fn scalars(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.walk(&mut out);
        out
    }

    fn walk<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Val::Atom(a) => out.push(a),
            Val::Seq(xs) => {
                for x in xs {
                    x.walk(out);
                }
            }
            Val::Opaque => {}
        }
    }

    fn floats(&self) -> Vec<f64> {
        self.scalars().iter().filter_map(|s| s.parse().ok()).collect()
    }

    /// Pfad-Notation `</a/b>` normalisieren (ohne spitze Klammern, ohne führende `/`).
    fn path(&self) -> Option<String> {
        let a = match self {
            Val::Atom(a) => a.as_str(),
            _ => return None,
        };
        let t = a.trim_matches('<').trim_matches('>');
        if t.is_empty() {
            return None;
        }
        Some(t.trim_start_matches('/').to_string())
    }
}

#[derive(Debug)]
struct Attr {
    name: String,
    val: Option<Val>,
}

#[derive(Debug)]
struct MetaEntry {
    /// Schlüssel ohne Fallback-Klammern, z. B. `material:binding`.
    key: String,
    val: Option<Val>,
}

#[derive(Debug)]
struct Prim {
    ty: String, // "" bei untypisiertem `def "Name"`
    name: String,
    meta: Vec<MetaEntry>,
    rels: Vec<(String, Option<Val>)>,
    attrs: Vec<Attr>,
    children: Vec<Prim>,
}

impl Prim {
    fn attr(&self, name: &str) -> Option<&Val> {
        self.attrs
            .iter()
            .find(|a| a.name == name)
            .and_then(|a| a.val.as_ref())
    }
}

// ───────────────────────────── Parser ────────────────────────────────

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn word(&mut self) -> Result<String> {
        match self.peek() {
            Some(Tok::Word(w)) => {
                let w = w.clone();
                self.i += 1;
                Ok(w)
            }
            other => bail!("USD: Bezeichner erwartet, gefunden {other:?}"),
        }
    }

    fn string(&mut self) -> Result<String> {
        match self.peek() {
            Some(Tok::Str(s)) => {
                let s = s.clone();
                self.i += 1;
                Ok(s)
            }
            other => bail!("USD: String erwartet, gefunden {other:?}"),
        }
    }

    fn eat_punct(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, c: char) -> Result<()> {
        if self.eat_punct(c) {
            Ok(())
        } else {
            bail!("USD: {c:?} erwartet, gefunden {:?}", self.peek())
        }
    }

    /// `( … )`-Metadaten-Block. Referenzen/Payloads/Varianten: klarer Abbruch.
    fn meta_block(&mut self) -> Result<Vec<MetaEntry>> {
        self.expect_punct('(')?;
        let mut out = Vec::new();
        loop {
            if self.eat_punct(')') {
                return Ok(out);
            }
            if self.eat_punct(';') || self.eat_punct(',') {
                continue;
            }
            let raw = self.word()?;
            let key = raw.split('<').next().unwrap_or("").to_string();
            if self.eat_punct('=') {
                let v = self.value()?;
                match key.as_str() {
                    "references" | "payloads" => bail!(
                        "USD: externe Referenzen (references/payloads) sind außerhalb der Teilmenge"
                    ),
                    "variantSets" => bail!("USD: Varianten sind außerhalb der Teilmenge"),
                    _ => out.push(MetaEntry { key, val: Some(v) }),
                }
            } else {
                out.push(MetaEntry { key, val: None });
            }
        }
    }

    fn value(&mut self) -> Result<Val> {
        match self.peek().cloned() {
            Some(Tok::Punct('[')) | Some(Tok::Punct('(')) => {
                let close = if self.eat_punct('[') { ']' } else { self.expect_punct('(')?; ')' };
                let mut xs = Vec::new();
                while !self.eat_punct(close) {
                    if self.i >= self.t.len() {
                        bail!("USD: Value-Sequenz nicht geschlossen");
                    }
                    if self.eat_punct(',') {
                        continue;
                    }
                    xs.push(self.value()?);
                }
                Ok(Val::Seq(xs))
            }
            Some(Tok::Punct('{')) => {
                // Dictionary-Metadaten (customLayerData …): balanciert überspringen.
                self.i += 1;
                let mut depth = 1usize;
                while depth > 0 {
                    match self.peek() {
                        None => bail!("USD: Dictionary nicht geschlossen"),
                        Some(Tok::Punct('{')) => depth += 1,
                        Some(Tok::Punct('}')) => depth -= 1,
                        _ => {}
                    }
                    self.i += 1;
                }
                Ok(Val::Opaque)
            }
            Some(Tok::Str(s)) => {
                self.i += 1;
                Ok(Val::Atom(s))
            }
            Some(Tok::Word(w)) => {
                self.i += 1;
                Ok(Val::Atom(w))
            }
            other => bail!("USD: Wert erwartet, gefunden {other:?}"),
        }
    }

    /// Body eines Prim-Blocks bis zum abschließenden `}`.
    fn prim_body(&mut self, prim: &mut Prim) -> Result<()> {
        loop {
            match self.peek() {
                None => bail!("USD: Prim-Block ohne schließende Klammer"),
                Some(Tok::Punct('}')) => {
                    self.i += 1;
                    return Ok(());
                }
                Some(Tok::Punct(';')) => {
                    self.i += 1;
                }
                Some(Tok::Str(s)) => {
                    bail!("USD: String {s:?} statt Anweisung im Prim-Block");
                }
                Some(Tok::Punct(c)) => {
                    bail!("USD: unerwartetes {c:?} im Prim-Block");
                }
                Some(Tok::Word(w)) => {
                    let w = w.clone();
                    self.i += 1;
                    match w.as_str() {
                        "def" | "over" | "class" => {
                            let child = self.prim(&w)?;
                            prim.children.push(child);
                        }
                        "variantSet" | "variantsets" | "variants" => {
                            bail!("USD: Varianten sind außerhalb der Teilmenge");
                        }
                        "rel" => {
                            let raw = self.word()?;
                            let key = raw.split('<').next().unwrap_or("").to_string();
                            if key == "variantSets" {
                                bail!("USD: Varianten sind außerhalb der Teilmenge");
                            }
                            let val = if self.eat_punct('=') { Some(self.value()?) } else { None };
                            self.eat_punct(';');
                            prim.rels.push((key, val));
                        }
                        "uniform" | "custom" => self.attribute_with_type(prim)?,
                        _ => self.attribute_after_type_word(prim, w)?,
                    }
                }
            }
        }
    }

    /// Nach `uniform`/`custom`: Typ [+`[]`], Name, optionale Metadaten, Wert.
    fn attribute_with_type(&mut self, prim: &mut Prim) -> Result<()> {
        let ty = self.word()?;
        if self.eat_punct('[') {
            self.expect_punct(']')?;
        }
        let name = self.word()?;
        self.attribute_rest(prim, ty, name)
    }

    /// Ein Wort, das keine Anweisung war, ist der Typnamen-Anfang eines Attributs.
    fn attribute_after_type_word(&mut self, prim: &mut Prim, ty: String) -> Result<()> {
        if self.eat_punct('[') {
            self.expect_punct(']')?;
        }
        let name = self.word()?;
        self.attribute_rest(prim, ty, name)
    }

    /// Nach `typ name`: optionale Attribut-Metadaten, optionales `= Wert`.
    /// Der Typname wird geparst, aber nicht ausgewertet — die Werteform
    /// unterscheidet die Formen ohnehin, und unbekannte Typen sollen nicht
    /// den Import blockieren.
    fn attribute_rest(&mut self, prim: &mut Prim, ty: String, name: String) -> Result<()> {
        let _ = ty;
        if self.peek() == Some(&Tok::Punct('(')) {
            // Attribut-Metadaten (interpolation, hidden, componentNames …) — egal.
            let _ = self.meta_block()?;
        }
        let val = if self.eat_punct('=') { Some(self.value()?) } else { None };
        self.eat_punct(';');
        prim.attrs.push(Attr { name, val });
        Ok(())
    }

    /// `def`/`over`/`class` [Typ] "Name" [(Metadaten)] [{…}]
    fn prim(&mut self, kw: &str) -> Result<Prim> {
        let _ = kw;
        let ty = match self.peek() {
            Some(Tok::Word(_)) => self.word()?,
            _ => String::new(),
        };
        let name = self.string()?;
        let meta = if self.peek() == Some(&Tok::Punct('(')) { self.meta_block()? } else { Vec::new() };
        let mut prim = Prim { ty, name, meta, rels: Vec::new(), attrs: Vec::new(), children: Vec::new() };
        if self.eat_punct('{') {
            self.prim_body(&mut prim)?;
        }
        Ok(prim)
    }

    fn file(&mut self) -> Result<(Vec<Prim>, Vec<MetaEntry>)> {
        let mut prims = Vec::new();
        let mut stage_meta = Vec::new();
        loop {
            match self.peek() {
                None => return Ok((prims, stage_meta)),
                Some(Tok::Punct('(')) => {
                    let m = self.meta_block()?;
                    stage_meta.extend(m);
                }
                Some(Tok::Word(w)) if matches!(w.as_str(), "def" | "over" | "class") => {
                    let w = w.clone();
                    self.i += 1;
                    prims.push(self.prim(&w)?);
                }
                Some(Tok::Word(w)) if w == "version" => {
                    self.i += 1;
                    if let Some(Tok::Word(_)) = self.peek() {
                        self.i += 1;
                    }
                }
                other => bail!("USD: Wurzel-`def` erwartet, gefunden {other:?}"),
            }
        }
    }
}

// ───────────────────────── Projekt-Aufbau ────────────────────────────

struct Builder<'a> {
    mats: HashMap<String, MaterialId>,
    proj: &'a mut Project,
}

fn child_path(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    }
}

impl<'a> Builder<'a> {
    fn scalars(&self, prim: &Prim, name: &str) -> Vec<f64> {
        prim.attr(name).map(|v| v.floats()).unwrap_or_default()
    }

    fn bool_attr(prim: &Prim, name: &str) -> Option<bool> {
        let a = prim.attr(name)?.scalars();
        match a.first()?.to_ascii_lowercase().as_str() {
            "true" | "1" | "1.0" => Some(true),
            "false" | "0" | "0.0" => Some(false),
            _ => None,
        }
    }

    /// Bindungs-Pfad eines Prims: `rel material:binding` oder Metadatum;
    /// sonst geerbte Bindung.
    fn binding_of(&self, prim: &Prim, inherited: Option<String>) -> Option<String> {
        for (key, val) in
            prim.rels.iter().map(|(k, v)| (k, v)).chain(prim.meta.iter().map(|m| (&m.key, &m.val)))
        {
            if key == "material:binding" {
                if let Some(p) = val.as_ref().and_then(|v| v.path()) {
                    return Some(p);
                }
            }
        }
        inherited
    }

    fn color3(v: &Val) -> Option<[f32; 3]> {
        let f = v.floats();
        match f.len() {
            0 => None,
            1 => Some([f[0] as f32; 3]),
            _ => Some([f[0] as f32, f[1] as f32, f[2] as f32]),
        }
    }

    /// Pass 1: Material-Prims einsammeln — Bindungen zeigen per Pfad auf sie,
    /// und der Pfad ist vor dem Geometrie-Aufbau vollständig bekannt.
    fn collect_materials(&mut self, prims: &[Prim], base: &str) -> Result<()> {
        for p in prims {
            let path = child_path(base, &p.name);
            if p.ty == "Material" {
                let mat = self.build_material(p)?;
                let id = self.proj.library.add_material(mat);
                self.mats.insert(path, id);
                continue; // Shader-Interna sind keine Szenenknoten
            }
            self.collect_materials(&p.children, &path)?;
        }
        Ok(())
    }

    fn build_material(&mut self, prim: &Prim) -> Result<Material> {
        let mut m = Material { name: prim.name.clone(), ..Default::default() };
        m.double_sided = Self::bool_attr(prim, "doubleSided").unwrap_or(false);
        let mut albedo: Option<String> = None;
        for sh in &prim.children {
            let id = sh
                .attr("info:id")
                .and_then(|v| v.scalars().first().map(|s| s.to_string()))
                .unwrap_or_default();
            match id.as_str() {
                "UsdPreviewSurface" => {
                    if let Some(v) = sh.attr("inputs:diffuseColor").or_else(|| sh.attr("inputs:color")) {
                        if let Some(c) = Self::color3(v) {
                            m.base_color[..3].copy_from_slice(&c);
                        }
                    }
                    if let Some(v) = sh.attr("inputs:opacity") {
                        if let Some(x) = v.floats().first() {
                            m.base_color[3] = *x as f32;
                        }
                    }
                    if let Some(v) = sh.attr("inputs:metallic") {
                        if let Some(x) = v.floats().first() {
                            m.metallic = *x as f32;
                        }
                    }
                    if let Some(v) = sh.attr("inputs:roughness") {
                        if let Some(x) = v.floats().first() {
                            m.roughness = *x as f32;
                        }
                    }
                    if let Some(v) = sh.attr("inputs:emissionColor") {
                        if let Some(c) = Self::color3(v) {
                            m.emissive = c;
                        }
                    }
                }
                "UsdUVTexture" => {
                    if albedo.is_none() {
                        let a = sh
                            .attr("file")
                            .or_else(|| sh.attr("inputs:file"))
                            .map(|v| v.scalars())
                            .unwrap_or_default();
                        if let Some(f) = a.first() {
                            albedo = Some(f.trim_matches('@').replace('\\', "/"));
                        }
                    }
                }
                _ => {}
            }
        }
        m.albedo_texture_id = albedo;
        Ok(m)
    }

    /// Transform aus der xformOp-Resequenz; ohne Ops: Identität.
    fn local_transform(&self, prim: &Prim) -> Result<Transform> {
        let order_val = prim.attr("xformOpOrder").or_else(|| prim.attr("xformOps"));
        let ops: Vec<String> = match order_val {
            Some(v) => v.scalars().iter().map(|s| s.to_string()).collect(),
            None => prim
                .attrs
                .iter()
                .filter(|a| a.name == "xformOp" || a.name.starts_with("xformOp:"))
                .map(|a| a.name.clone())
                .collect(),
        };
        if ops.is_empty() {
            return Ok(Transform::IDENTITY);
        }
        let mut m = Mat4::IDENTITY;
        for op in ops {
            if op == "resetXformOrder" || op == "!resetXformStack!" {
                // Wir starten ohnehin bei Identität — erben ist Engine-Standard.
                continue;
            }
            let (name, invert) = match op.strip_prefix("!invert!") {
                Some(rest) => (rest.to_string(), true),
                None => (op.clone(), false),
            };
            let mut op_m = self.op_matrix(prim, &name).with_context(|| format!("USD: {name}"))?;
            if invert {
                if op_m.determinant().abs() < 1e-8 {
                    bail!("USD: !invert! auf nicht invertierbarer Matrix in {name}");
                }
                op_m = op_m.inverse();
            }
            m = m * op_m;
        }
        Ok(Transform::from_mat4(&m).unwrap_or_default())
    }

    /// Op-Kern ohne Namensraum und Suffix: `xformOp:translate:A1` → `translate`.
    fn op_kind(op_name: &str) -> &str {
        if op_name == "xformOp" {
            return "matrix";
        }
        match op_name.strip_prefix("xformOp:") {
            Some(rest) => rest.split(':').next().unwrap_or(rest),
            None => op_name,
        }
    }

    fn op_matrix(&self, prim: &Prim, op_name: &str) -> Result<Mat4> {
        let kind = Self::op_kind(op_name);
        // Werte liegen im Attrib mit dem vollen Operationsnamen (inkl. Suffix).
        let nums: Vec<f64> = prim.attr(op_name).map(|x| x.floats()).unwrap_or_default();
        let vec3 = || -> Result<[f32; 3]> {
            match nums.len() {
                1 => Ok([nums[0] as f32; 3]),
                n if n >= 3 => Ok([nums[0] as f32, nums[1] as f32, nums[2] as f32]),
                _ => bail!("drei Zahlen erwartet, gefunden {:?}", nums),
            }
        };
        let one = || -> Result<f32> {
            nums.first().copied().map(|x| x as f32).ok_or_else(|| anyhow::anyhow!("Zahl erwartet"))
        };
        let mat = match kind {
            "matrix" | "transform" => {
                if nums.len() != 16 {
                    if kind == "transform" {
                        // `xformOp:transform = </Ns>` verweist auf einen Namespace-Prim.
                        bail!("Referenz-Transforms sind außerhalb der Teilmenge");
                    }
                    bail!("matrix4d braucht 16 Werte");
                }
                // Gf speichert zeilenmajor mit Übersetzung in der letzten Zeile
                // (Zeilenvektor-Konvention). Als Spaltenmajor-Array interpretiert
                // entsteht automatisch die Konjugation in Spaltenmathematik.
                let mut f = [0f32; 16];
                f.copy_from_slice(&nums.iter().map(|x| *x as f32).collect::<Vec<f32>>());
                Mat4::from_cols_array(&f)
            }
            "translate" => Mat4::from_translation(Vec3::from_array(vec3()?)),
            "translateX" => Mat4::from_translation(Vec3::new(one()?, 0.0, 0.0)),
            "translateY" => Mat4::from_translation(Vec3::new(0.0, one()?, 0.0)),
            "translateZ" => Mat4::from_translation(Vec3::new(0.0, 0.0, one()?)),
            "scale" => Mat4::from_scale(Vec3::from_array(vec3()?)),
            "scaleX" => Mat4::from_scale(Vec3::new(one()?, 1.0, 1.0)),
            "scaleY" => Mat4::from_scale(Vec3::new(1.0, one()?, 1.0)),
            "scaleZ" => Mat4::from_scale(Vec3::new(1.0, 1.0, one()?)),
            "orient" => {
                if nums.len() < 4 {
                    bail!("quat erwartet vier Werte (w, x, y, z)");
                }
                let q =
                    Quat::from_xyzw(nums[1] as f32, nums[2] as f32, nums[3] as f32, nums[0] as f32)
                        .normalize();
                Mat4::from_quat(q)
            }
            other if other.starts_with("rotate") => {
                let axis_quat = |c: char| -> Result<Quat> {
                    let idx = match c {
                        'X' => 0,
                        'Y' => 1,
                        'Z' => 2,
                        _ => bail!("unbekannte Rotationsachse {c:?} in {op_name}"),
                    };
                    if nums.len() <= idx {
                        bail!("Rotationswert fehlt in {op_name}");
                    }
                    let a = (nums[idx] as f32).to_radians();
                    Ok(match c {
                        'X' => Quat::from_rotation_x(a),
                        'Y' => Quat::from_rotation_y(a),
                        _ => Quat::from_rotation_z(a),
                    })
                };
                let spec = &other["rotate".len()..];
                let q = match spec {
                    "X" | "Y" | "Z" => axis_quat(spec.chars().next().unwrap())?,
                    s if s.len() == 3 && s.is_ascii() => {
                        // Gf verzehrt die Achsen in Listenreihenfolge a→b→c
                        // (extrinsisch um feste Weltachsen); in Spaltenmathematik
                        // ist das q = q_c · q_b · q_a.
                        let cs: Vec<char> = s.chars().collect();
                        axis_quat(cs[2])? * axis_quat(cs[1])? * axis_quat(cs[0])?
                    }
                    _ => bail!("unbekannte Rotationsoperation {op_name}"),
                };
                Mat4::from_quat(q)
            }
            _ => bail!("unbekannte Transform-Operation {op_name:?}"),
        };
        Ok(mat)
    }

    /// Pass 2: Hierarchie + Meshes; Material-Unteräume werden übersprungen.
    fn build(
        &mut self,
        prims: &[Prim],
        base: &str,
        parent: Option<NodeId>,
        binding: Option<String>,
    ) -> Result<()> {
        for p in prims {
            let path = child_path(base, &p.name);
            if p.ty == "Material" {
                continue; // bereits in Pass 1 verarbeitet
            }
            let my_binding = self.binding_of(p, binding.clone());
            let kind = if p.ty == "Mesh" && p.attr("points").is_some() {
                let mesh = self.build_mesh(p)?;
                let mat = my_binding.as_ref().and_then(|b| self.mats.get(b).copied());
                NodeKind::MeshInstance { mesh, material: mat, skeleton_node: None }
            } else {
                NodeKind::Empty
            };
            let node = WNode {
                name: p.name.clone(),
                transform: self.local_transform(p)?,
                kind,
                ..Default::default()
            };
            let id = self.proj.scene.insert(parent, node);
            self.build(&p.children, &path, Some(id), my_binding)?;
        }
        Ok(())
    }

    fn build_mesh(&mut self, prim: &Prim) -> Result<re_core::id::MeshId> {
        let pts = prim.attr("points").context("points fehlt")?.floats();
        if pts.is_empty() || pts.len() % 3 != 0 {
            bail!("points muss ein Vielfaches von drei Zahlen sein");
        }
        let positions: Vec<[f32; 3]> =
            pts.chunks_exact(3).map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect();

        let idx_raw = self.scalars(prim, "faceVertexIndices");
        let counts_raw = self.scalars(prim, "faceVertexCounts");
        let n = positions.len();
        let mut indices: Vec<u32> = Vec::new();
        if counts_raw.is_empty() {
            if idx_raw.is_empty() {
                if n % 3 != 0 {
                    bail!("Mesh ohne faceVertexIndices: Punktzahl {n} ist kein Dreiecksraster");
                }
                indices = (0..n as u32).collect();
            } else {
                if idx_raw.len() % 3 != 0 {
                    bail!("faceVertexIndices ohne faceVertexCounts: Länge ist kein Dreiecks-Vielfaches");
                }
                indices = idx_raw.iter().map(|v| *v as u32).collect();
            }
        } else {
            // Polygonzüge: Fächer um den jeweiligen ersten Eckpunkt.
            let mut off = 0usize;
            for c in &counts_raw {
                let len = *c as usize;
                if len < 3 || off + len > idx_raw.len() {
                    bail!("faceVertexCounts ({len}) passt nicht zu faceVertexIndices");
                }
                let poly: Vec<u32> = idx_raw[off..off + len].iter().map(|v| *v as u32).collect();
                for i in 1..len - 1 {
                    indices.extend_from_slice(&[poly[0], poly[i], poly[i + 1]]);
                }
                off += len;
            }
        }
        for i in &indices {
            if *i as usize >= n {
                bail!("Index {i} außerhalb der Punktzahl {n}");
            }
        }
        // USD-Standard ist rechtshändig (CCW); „leftHanded“ kehrt um.
        let handed = prim
            .attr("orientation")
            .and_then(|v| v.scalars().first().map(|s| s.trim_matches('"').to_string()))
            .unwrap_or_else(|| "rightHanded".into());
        if handed == "leftHanded" {
            indices.chunks_exact_mut(3).for_each(|t| t.swap(1, 2));
        }

        let normals = self
            .flat_attr_matching(prim, "normals", n, 3)
            .map(|f| f.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect::<Vec<[f32; 3]>>());
        let uvs = self
            .flat_attr_matching(prim, "primvars:st", n, 2)
            .map(|f| f.chunks_exact(2).map(|c| [c[0], c[1]]).collect::<Vec<[f32; 2]>>());

        let mesh = WMesh {
            name: prim.name.clone(),
            positions,
            normals,
            uvs,
            indices: Some(indices),
            skin: None,
            stamp: re_core::mesh::new_mesh_stamp(),
        };
        Ok(self.proj.library.add_mesh(mesh))
    }

    /// Flache Zahlenliste eines Attributs, wenn sie exakt n·k Components hat.
    fn flat_attr_matching(&self, prim: &Prim, name: &str, n: usize, k: usize) -> Option<Vec<f32>> {
        let f = self.scalars(prim, name);
        if !f.is_empty() && f.len() == n * k {
            return Some(f.iter().map(|x| *x as f32).collect());
        }
        None
    }
}

// ───────────────────────────── Einstieg ──────────────────────────────

/// Liest `.usda`-Inhalt und baut das Projekt (Name: Dateistamm).
pub fn import(path: &Path) -> Result<Project> {
    let bytes = std::fs::read(path).with_context(|| format!("open {}", path.display()))?;
    let text = if bytes.starts_with(b"\xEF\xBB\xBF") { &bytes[3..] } else { &bytes[..] };
    if text.starts_with(b"PXR-USDC") {
        bail!(
            "USD: {} ist binäres .usdc — die Teilmenge liest nur ASCII-USD (.usda); \
             bitte mit `usdcat -o` in Textform umwandeln",
            path.display()
        );
    }
    let text = std::str::from_utf8(text)
        .with_context(|| format!("{} ist kein UTF-8-Text (binäres USD?)", path.display()))?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("imported");
    parse_to_project(text, stem).with_context(|| format!("parse {}", path.display()))
}

/// Parst USD-ASCII-Text direkt (auch für Tests und den Agentur-Pfad später).
pub fn parse_to_project(text: &str, name_hint: &str) -> Result<Project> {
    let toks = tokenize(text)?;
    let mut p = Parser { t: toks, i: 0 };
    let (prims, stage_meta) = p.file()?;
    if prims.is_empty() {
        bail!("USD: keine Prims gefunden — leere oder unverständliche Datei");
    }

    let mut proj = Project::new(name_hint);
    // upAxis = "Z" (Maya/Houdini-Exporte) → Korrekturknoten wie bei FBX.
    let up_z = stage_meta.iter().any(|m| {
        m.key == "upAxis"
            && m.val
                .as_ref()
                .and_then(|v| v.scalars().first().map(|s| *s == "Z"))
                .unwrap_or(false)
    });
    let root_parent = if up_z {
        let q = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        Some(proj.scene.insert(
            None,
            WNode {
                name: "USD (Z-up fix)".into(),
                transform: Transform {
                    translation: [0.0; 3],
                    rotation: [q.x, q.y, q.z, q.w],
                    scale: [1.0; 3],
                },
                kind: NodeKind::Empty,
                ..Default::default()
            },
        ))
    } else {
        None
    };

    let mut b = Builder { mats: HashMap::new(), proj: &mut proj };
    b.collect_materials(&prims, "")?;
    b.build(&prims, "", root_parent, None)?;
    Ok(proj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use re_core::id::NodeId;

    fn parse(s: &str) -> Result<Project> {
        parse_to_project(s, "test")
    }

    fn find_node(p: &Project, name: &str) -> NodeId {
        p.scene
            .nodes
            .iter()
            .find(|(_, n)| n.name == name)
            .map(|(id, _)| *id)
            .expect("Knoten nicht gefunden")
    }

    fn mesh_of<'a>(p: &'a Project, node: NodeId) -> &'a WMesh {
        match &p.scene.get(node).unwrap().kind {
            NodeKind::MeshInstance { mesh, .. } => p.library.meshes.get(mesh).unwrap(),
            other => panic!("kein Mesh-Knoten: {other:?}"),
        }
    }

    #[test]
    fn hierarchie_und_leere_container() {
        let p = parse(
            r#"
            #usda 1.0
            def Xform "World"
            {
                def Scope "Group"
                {
                    def "Bare" { }
                }
            }
            "#,
        )
        .unwrap();
        assert!(p.scene.nodes.len() == 3);
        assert!(p.scene.roots.len() == 1);
        let world = find_node(&p, "World");
        assert_eq!(p.scene.get(world).unwrap().children.len(), 1);
        // Untypisierter Prim bleibt als leerer Knoten erhalten.
        find_node(&p, "Bare");
    }

    #[test]
    fn polygon_faecher_und_winding() {
        let p = parse(
            r#"
            def Mesh "Quad"
            {
                point3f[] points = [(0,0,0), (1,0,0), (1,1,0), (0,1,0)]
                int[] faceVertexCounts = [4]
                int[] faceVertexIndices = [0, 1, 2, 3]
            }
            "#,
        )
        .unwrap();
        let m = mesh_of(&p, find_node(&p, "Quad"));
        // Fächer: (0,1,2) und (0,2,3)
        assert_eq!(m.indices.as_deref(), Some(&[0u32, 1, 2, 0, 2, 3][..]));
    }

    #[test]
    fn left_handed_kehrt_winding() {
        let p = parse(
            r#"
            def Mesh "Tri"
            {
                point3f[] points = [(0,0,0), (1,0,0), (0,1,0)]
                uniform token orientation = "leftHanded"
                int[] faceVertexIndices = [0, 1, 2]
            }
            "#,
        )
        .unwrap();
        let m = mesh_of(&p, find_node(&p, "Tri"));
        assert_eq!(m.indices.as_deref(), Some(&[0u32, 2, 1][..]));
    }

    #[test]
    fn normals_und_st_bei_passender_zahl() {
        let p = parse(
            r#"
            def Mesh "Tri"
            {
                point3f[] points = [(0,0,0), (1,0,0), (0,1,0)]
                normal3f[] normals = [(0,0,1), (0,0,1), (0,0,1)]
                point2f[] primvars:st = [(0,0), (1,0), (0,1)]
                int[] faceVertexIndices = [0, 1, 2]
            }
            "#,
        )
        .unwrap();
        let m = mesh_of(&p, find_node(&p, "Tri"));
        assert_eq!(m.normals.as_ref().map(|v| v.len()), Some(3));
        assert_eq!(m.uvs.as_ref().map(|v| v.len()), Some(3));
    }

    #[test]
    fn face_varying_attribut_faellt_herunter() {
        let p = parse(
            r#"
            def Mesh "Tri"
            {
                point3f[] points = [(0,0,0), (1,0,0), (0,1,0)]
                normal3f[] normals = [(0,0,1), (0,0,1), (0,0,1), (0,0,1), (0,0,1), (0,0,1)]
                int[] faceVertexIndices = [0, 1, 2]
            }
            "#,
        )
        .unwrap();
        let m = mesh_of(&p, find_node(&p, "Tri"));
        assert!(m.normals.is_none());
    }

    #[test]
    fn xform_op_reihenfolge_translate_rotate_scale() {
        let p = parse(
            r#"
            def Xform "A"
            {
                uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"]
                float3 xformOp:translate = (2, 0, 0)
                float3 xformOp:rotateXYZ = (0, 90, 0)
                float3 xformOp:scale = (2, 2, 2)
            }
            "#,
        )
        .unwrap();
        let m = p.scene.world_transform(find_node(&p, "A")).to_mat4();
        let v = m.transform_point3(Vec3::X);
        // Anwenden gegen die Listenreihenfolge: erst skalieren (2,0,0),
        // dann Y90 (→ (0,0,−2)), dann +T.
        assert!((v - Vec3::new(2.0, 0.0, -2.0)).length() < 1e-5, "gefunden {v}");
    }

    #[test]
    fn rotate_xyz_extrinsisch() {
        // Gf: rotateXYZ verzehrt (90, 0, 90) als X dann Z um feste Weltachsen:
        // +Z → (X90) → (0,−1,0) → (Z90) → (1,0,0).
        let p = parse(
            r#"
            def Xform "A"
            {
                uniform token[] xformOpOrder = ["xformOp:rotateXYZ"]
                float3 xformOp:rotateXYZ = (90, 0, 90)
            }
            "#,
        )
        .unwrap();
        let m = p.scene.world_transform(find_node(&p, "A")).to_mat4();
        let v = m.transform_point3(Vec3::Z);
        assert!((v - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5, "gefunden {v}");
    }

    #[test]
    fn orient_quaternion_w_zuerst() {
        let p = parse(
            r#"
            def Xform "A"
            {
                uniform token[] xformOpOrder = ["xformOp:orient"]
                quatd xformOp:orient = (0.7071067811865476, 0.0, 0.0, 0.7071067811865476)
            }
            "#,
        )
        .unwrap();
        let m = p.scene.world_transform(find_node(&p, "A")).to_mat4();
        let v = m.transform_point3(Vec3::X);
        assert!((v - Vec3::Y).length() < 1e-5, "gefunden {v}");
    }

    #[test]
    fn matrix4d_zeilenmajor() {
        let p = parse(
            r#"
            def Xform "A"
            {
                matrix4d xformOp = [(1,0,0,0), (0,1,0,0), (0,0,1,0), (5,6,7,1)]
            }
            "#,
        )
        .unwrap();
        let m = p.scene.world_transform(find_node(&p, "A")).to_mat4();
        assert_eq!(m.transform_point3(Vec3::ZERO), Vec3::new(5.0, 6.0, 7.0));
    }

    #[test]
    fn invert_op() {
        let p = parse(
            r#"
            def Xform "A"
            {
                uniform token[] xformOpOrder = ["!invert!xformOp:translate"]
                float3 xformOp:translate = (3, 0, 0)
            }
            "#,
        )
        .unwrap();
        let m = p.scene.world_transform(find_node(&p, "A")).to_mat4();
        assert_eq!(m.transform_point3(Vec3::ZERO), Vec3::new(-3.0, 0.0, 0.0));
    }

    #[test]
    fn materialbindung_vererbt_und_trifft_mesh() {
        let p = parse(
            r#"
            def Material "Rot"
            {
                bool doubleSided = 1
                def Shader "Preview"
                {
                    uniform token info:id = "UsdPreviewSurface"
                    color3f inputs:diffuseColor = (1, 0, 0)
                    float inputs:opacity = 0.5
                    float inputs:roughness = 0.25
                }
            }
            def Xform "Holder" (
                material:binding = </MaterialNotUsed>
            )
            {
                rel material:binding = </Rot>
                def Mesh "M"
                {
                    point3f[] points = [(0,0,0), (1,0,0), (0,1,0)]
                    int[] faceVertexIndices = [0, 1, 2]
                }
            }
            "#,
        )
        .unwrap();
        let node = find_node(&p, "M");
        let mat = match &p.scene.get(node).unwrap().kind {
            NodeKind::MeshInstance { material: Some(id), .. } => p.library.materials.get(id).unwrap(),
            other => panic!("erwartete gebundenes Mesh, gefunden {other:?}"),
        };
        assert_eq!(mat.base_color, [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(mat.roughness, 0.25);
        assert!(mat.double_sided);
    }

    #[test]
    fn uv_textur_albedo() {
        let p = parse(
            r#"
            def Material "Tex"
            {
                def Shader "Blinn"
                {
                    uniform token info:id = "UsdPreviewSurface"
                    color3f inputs:diffuseColor = (1, 1, 1)
                }
                def Shader "Bild"
                {
                    uniform token info:id = "UsdUVTexture"
                    asset file = @holz/rinde.png@
                }
            }
            "#,
        )
        .unwrap();
        let mat = p.library.materials.values().next().unwrap();
        assert_eq!(mat.albedo_texture_id.as_deref(), Some("holz/rinde.png"));
    }

    #[test]
    fn z_up_korrekturknoten() {
        let p = parse(
            r#"
            #usda 1.0
            (
                upAxis = "Z"
                metersPerUnit = 0.01
            )
            def Xform "World"
            {
                def "Kind" { }
            }
            "#,
        )
        .unwrap();
        assert_eq!(p.scene.roots.len(), 1);
        let root = p.scene.get(p.scene.roots[0]).unwrap();
        assert_eq!(root.name, "USD (Z-up fix)");
        assert_eq!(root.children.len(), 1);
    }

    #[test]
    fn referenzen_melden_klar() {
        let e = parse(
            r#"
            def Xform "A" (
                references = @other.usda@
            )
            { }
            "#,
        )
        .unwrap_err();
        assert!(e.to_string().contains("Referenzen"), "klare Ansage: {e}");
    }

    #[test]
    fn varianten_melden_klar() {
        let e = parse(
            r#"
            def Xform "A"
            {
                variantSet "lod" { }
            }
            "#,
        )
        .unwrap_err();
        assert!(e.to_string().contains("Variante"), "klare Ansage: {e}");
    }

    #[test]
    fn referenz_transform_meldet_klar() {
        let e = parse(
            r#"
            def Xform "A"
            {
                uniform token[] xformOpOrder = ["xformOp:transform"]
                matrix4d xformOp:transform = </World/Ns>
            }
            "#,
        )
        .unwrap_err();
        let msg = format!("{e:#}"); // volle Fehlerkette inkl. Kontext
        assert!(msg.contains("Referenz-Transform"), "klare Ansage: {msg}");
    }

    #[test]
    fn dictionary_metadaten_stoeren_nicht() {
        let p = parse(
            r#"
            (
                customLayerData = {
                    string creator = "test"
                    dictionary inner = { int x = 1 }
                }
            )
            def Xform "A" { }
            "#,
        )
        .unwrap();
        assert_eq!(p.scene.nodes.len(), 1);
    }
}
