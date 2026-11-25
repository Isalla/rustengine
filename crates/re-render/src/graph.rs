//! Render-Graph (M5, Box 5): Pässe und Ressourcen als Datenstruktur.
//!
//! Der Graph ist die Antwort auf die Frage „was muss in diesem Frame
//! passieren, in welcher Reihenfolge, und in welchem Zustand darf eine
//! Textur wann gelesen werden?". Er ist bewusst **backend-neutral und
//! deterministisch**: gleiche Eingabe → gleicher sortierter Ablauf,
//! gleiche Transitionen. Damit ist er headless testbar, bevor `re-wgpu`
//! ihn ausführt, und die Deferred-Pipeline (G-Buffer → Lichter →
//! temporale Filter) wird als Daten beschreibbar statt als Code-Zettel.
//!
//! Kernideen:
//! * Pässe deklarieren ihre Ressourcen- Zugriffe ([`Access`]) — der
//!   Graph, nicht der Pass, entscheidet über Reihenfolge und Layout.
//! * [`RenderGraph::compile`] sortiert topologisch (deterministischer
//!   Tie-Break über den Pass-Namen), verwirft ungenutzte Pässe und
//!   berechnet pro Pass die Layout-Transitionen aus den Nachbarn.
//! * Der Speicher-Aliasing-Schritt (Texturen teilen sich Bytes, wenn
//!   ihre Lebensspannen sich nicht überschneiden) folgt, sobald der
//!   Allocator in `re-wgpu` landet — hier wird nur die Lebensspanne
//!   ([`ResourceSpan`]) berechnet, die er braucht.

/// Eine Graph-Ressource (Textur oder Buffer) mit ihrem Start-Zustand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceDesc {
    pub name: String,
    /// Zustand vor dem ersten Zugriff (z. B. [`Layout::Undefined`] für
    /// eine frische Render-Target-Textur).
    pub initial: Layout,
}

/// Layout-Zustand einer Ressource — backend-neutral formuliert;
/// `re-wgpu` bildet das auf `wgpu::TextureUsage`/Load-Op abbilden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layout {
    /// Inhalt undefiniert — erster Zugriff muss schreiben/klaren.
    Undefined,
    /// Farb-Attachment (Color-Target).
    ColorAttachment,
    /// Tiefen-Attachment.
    DepthAttachment,
    /// Als Shader-Resource lesbar.
    ShaderRead,
    /// Ziel eines Kopiers.
    CopyDst,
    /// Quelle eines Kopiers.
    CopySrc,
    /// Direkt auf dem Swapchain sichtbar (letzter Pass des Frames).
    Present,
}

/// Wie ein Pass eine Ressource berührt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub resource: usize,
    pub kind: AccessKind,
}

/// Zugriffsart — bestimmt das Layout, das der Pass für die Ressource
/// braucht, und ob der Inhalt davor gültig sein muss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessKind {
    /// Schreibt die Ressource vollständig neu (Load-Op: Clear/DontCare).
    Write,
    /// Liest den Inhalt vorheriger Pässe.
    Read,
    /// Liest und schreibt an derselben Stelle (z. B. temporale
    /// History-Textur: Reprojizieren + Zurückschreiben).
    ReadWrite,
}

impl AccessKind {
    /// Layout, das dieser Zugriff verlangt.
    pub fn required_layout(self) -> Layout {
        match self {
            AccessKind::Write => Layout::ColorAttachment,
            AccessKind::Read => Layout::ShaderRead,
            // ReadWrite braucht schreibenden Zugriff; der Graph wählt
            // das Attachement-Layout, Lesen innerhalb eines Passes ist
            // in wgpu über dieselbe Bindung erlaubt (Storage-Textur
            // später als eigene Art, wenn die History im Compute liegt).
            AccessKind::ReadWrite => Layout::ColorAttachment,
        }
    }

    /// Muss der Inhalt beim Betreten des Passes gültig sein?
    pub fn needs_valid_input(self) -> bool {
        matches!(self, AccessKind::Read | AccessKind::ReadWrite)
    }
}

/// Ein Pass: Name + deklarierter Zugriffe + optionale explizite
/// Abhängigkeiten (Ordnung, die nicht aus Ressourcen-Flüssen folgt —
/// z. B. „UI nach TAA", obwohl beide keine gemeinsame Ressource teilen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassDesc {
    pub name: String,
    pub accesses: Vec<Access>,
    /// Pass-Indizes, die vor diesem laufen müssen.
    pub depends_on: Vec<usize>,
}

/// Lebensspanne einer Ressource im sortierten Ablauf — die Grundlage
/// für späteres Speicher-Aliasing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceSpan {
    /// Index in [`CompiledGraph::order`] des ersten zugreifenden Passes.
    pub first: usize,
    /// Index des letzten zugreifenden Passes.
    pub last: usize,
    /// Wird die Ressource nach dem letzten Zugriff noch gebraucht
    /// (über den Frame hinaus, z. B. History-Puffer)?
    pub persistent: bool,
}

/// Ein kompilierter, ausführbarer Ablauf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledGraph {
    /// Pass-Indizes (in den Vektoren des Graphen) in Ausführungsreihenfolge.
    pub order: Vec<usize>,
    /// Pro sortierter Pass-Stelle: Transitionen (Ressource, von, nach),
    /// die VOR dem Pass auszuführen sind.
    pub transitions: Vec<Vec<(usize, Layout, Layout)>>,
    /// Lebensspannen je Ressourcen-Index.
    pub spans: Vec<Option<ResourceSpan>>,
}

/// Fehler beim Kompilieren.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// Zugriff auf eine nicht existierende Ressource.
    UnknownResource { pass: String, resource: usize },
    /// Abhängigkeitszyklus — die genannten Pässe sind beteiligt.
    Cycle { passes: Vec<String> },
    /// Ein Pass liest, was in diesem Frame nie geschrieben wurde und
    /// auch nicht als gültig übergeben wird.
    UninitializedRead { pass: String, resource: String },
}

/// Der Graph in Bau-Form: Ressourcen und Pässe in Registrierungs-
/// Reihenfolge, noch unsortiert.
#[derive(Clone, Debug, Default)]
pub struct RenderGraph {
    resources: Vec<ResourceDesc>,
    passes: Vec<PassDesc>,
    /// Ressourcen, deren Inhalt vom letzten Frame mitgebracht wird
    /// (History, Hi-Z) — ihre Lebensspanne beginnt vor Pass 0.
    persistent: Vec<usize>,
}

impl RenderGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registriert eine Ressource, gibt ihren Index zurück.
    pub fn add_resource(&mut self, name: &str, initial: Layout) -> usize {
        self.resources.push(ResourceDesc { name: name.into(), initial });
        self.resources.len() - 1
    }

    /// Registriert eine Ressource, deren Inhalt den Frame überlebt
    /// (History-Texturen): Lesen im ersten Frame ist erlaubt.
    pub fn add_persistent(&mut self, name: &str, initial: Layout) -> usize {
        let i = self.add_resource(name, initial);
        self.persistent.push(i);
        i
    }

    /// Registriert einen Pass, gibt seinen Index zurück.
    pub fn add_pass(&mut self, name: &str, accesses: Vec<Access>) -> usize {
        self.passes.push(PassDesc { name: name.into(), accesses, depends_on: Vec::new() });
        self.passes.len() - 1
    }

    /// Registriert einen Pass mit expliziten Abhängigkeiten.
    pub fn add_pass_with_deps(
        &mut self,
        name: &str,
        accesses: Vec<Access>,
        depends_on: Vec<usize>,
    ) -> usize {
        self.passes.push(PassDesc { name: name.into(), accesses, depends_on });
        self.passes.len() - 1
    }

    pub fn resources(&self) -> &[ResourceDesc] {
        &self.resources
    }

    pub fn passes(&self) -> &[PassDesc] {
        &self.passes
    }

    /// Hilfsmittel: Zugriff-Eintrag bauen.
    pub fn access(resource: usize, kind: AccessKind) -> Access {
        Access { resource, kind }
    }

    /// Sortiert, entfernt ungenutzte Pässe, berechnet Transitionen und
    /// Lebensspannen. Deterministisch: gleiche Eingabe → byte-gleicher
    /// Ablauf (Tie-Break alphabetisch über den Pass-Namen).
    pub fn compile(&self) -> Result<CompiledGraph, GraphError> {
        // 1. Zugriff auf unbekannte Ressourcen abfangen.
        for p in &self.passes {
            for a in &p.accesses {
                if a.resource >= self.resources.len() {
                    return Err(GraphError::UnknownResource {
                        pass: p.name.clone(),
                        resource: a.resource,
                    });
                }
            }
        }

        // 2. Schreibende Pässe je Ressource (für Kanten Read→Writer).
        let mut writers: Vec<Vec<usize>> = vec![Vec::new(); self.resources.len()];
        for (pi, p) in self.passes.iter().enumerate() {
            for a in &p.accesses {
                if matches!(a.kind, AccessKind::Write | AccessKind::ReadWrite) {
                    writers[a.resource].push(pi);
                }
            }
        }

        // 3. Kanten: jeder Leser eines Werts hängt von jedem Schreiber
        //    derselben Ressource ab (Write/ReadWrite → Read), plus die
        //    expliziten depends_on-Kanten.
        let mut succ: Vec<Vec<usize>> = vec![Vec::new(); self.passes.len()];
        let mut indeg = vec![0usize; self.passes.len()];
        let add_edge = |w: usize, r: usize, succ: &mut Vec<Vec<usize>>, indeg: &mut Vec<usize>| {
            if w != r {
                succ[w].push(r);
                indeg[r] += 1;
            }
        };
        for (ri, ws) in writers.iter().enumerate() {
            for (pi, p) in self.passes.iter().enumerate() {
                let reads = p.accesses.iter().any(|a| {
                    a.resource == ri && matches!(a.kind, AccessKind::Read | AccessKind::ReadWrite)
                });
                if !reads {
                    continue;
                }
                for &w in ws {
                    add_edge(w, pi, &mut succ, &mut indeg);
                }
            }
        }
        for (pi, p) in self.passes.iter().enumerate() {
            for &d in &p.depends_on {
                add_edge(d, pi, &mut succ, &mut indeg);
            }
        }

        // 4. Kahn mit alphabetischem Tie-Break (Min-Heap über
        //    (Name, Reverse(Index))) — deterministisch ohne HashMap-Rest.
        let mut ready: std::collections::BinaryHeap<std::cmp::Reverse<(
            &str,
            std::cmp::Reverse<usize>,
        )>> = Default::default();
        for (i, d) in indeg.iter().enumerate() {
            if *d == 0 {
                ready.push(std::cmp::Reverse((
                    self.passes[i].name.as_str(),
                    std::cmp::Reverse(i),
                )));
            }
        }
        let mut order: Vec<usize> = Vec::with_capacity(self.passes.len());
        while let Some(std::cmp::Reverse((_, std::cmp::Reverse(i)))) = ready.pop() {
            order.push(i);
            for &s in &succ[i] {
                indeg[s] -= 1;
                if indeg[s] == 0 {
                    ready.push(std::cmp::Reverse((
                        self.passes[s].name.as_str(),
                        std::cmp::Reverse(s),
                    )));
                }
            }
        }
        if order.len() != self.passes.len() {
            let stuck: Vec<String> = (0..self.passes.len())
                .filter(|i| indeg[*i] > 0)
                .map(|i| self.passes[i].name.clone())
                .collect();
            return Err(GraphError::Cycle { passes: stuck });
        }

        // 5. Uninitialisierte Reads — VOR dem toten-Pass-Entfernen
        //    pruefen: ein Pass, der nur liest, waere sonst "tot" und
        //    der Fehler verschwindet stillschweigend. Jeder deklarierte
        //    Lesezugriff muss durch einen frueheren Schreiber gedeckt
        //    sein oder auf einer persistenten Ressource sitzen.
        let mut pos = vec![usize::MAX; self.passes.len()];
        for (o, &pi) in order.iter().enumerate() {
            pos[pi] = o;
        }
        for (o, &pi) in order.iter().enumerate() {
            for a in &self.passes[pi].accesses {
                if !a.kind.needs_valid_input() || self.persistent.contains(&a.resource) {
                    continue;
                }
                let covered = writers[a.resource].iter().any(|&w| pos[w] < o);
                if !covered {
                    return Err(GraphError::UninitializedRead {
                        pass: self.passes[pi].name.clone(),
                        resource: self.resources[a.resource].name.clone(),
                    });
                }
            }
        }

        // 6. Nutzbare Pässe: alles, was (direkt oder über Ketten) auf
        //    eine persistente Ressource oder eine Present-Ressource
        //    hinarbeitet, plus deren Transitive Hülle nach hinten.
        //    Hier: Rückwärts-Erreichbarkeit von „Senken" (Pässe, die
        //    persistentes/Present-Schreibgut erzeugen) — Pässe ohne
        //    Pfad zu einer Senke fallen weg.
        let mut sink = vec![false; self.passes.len()];
        for (pi, p) in self.passes.iter().enumerate() {
            for a in &p.accesses {
                let writes = matches!(a.kind, AccessKind::Write | AccessKind::ReadWrite);
                let wanted = self.persistent.contains(&a.resource)
                    || self.resources[a.resource].initial == Layout::Present;
                if writes && wanted {
                    sink[pi] = true;
                }
            }
        }
        // Present-Ressourcen: jeder Schreiber ist Senke, auch wenn der
        // Start-Zustand nicht Present heißt (der Swapchain wird es).
        // Präsenz erkennen wir über einen Pass, der Present liest? Nein —
        // Present ist ein Ziel-Layout, kein Start. Also: Pässe, die auf
        // eine Ressource schreiben, die später gelesen wird, sind nur
        // Senke, wenn die Ressource persistent ist. Zusätzlich: Pässe
        // ohne Nachfolger, die schreiben, sind Senken (Frame-Ende).
        for (pi, p) in self.passes.iter().enumerate() {
            let writes_something = p
                .accesses
                .iter()
                .any(|a| matches!(a.kind, AccessKind::Write | AccessKind::ReadWrite));
            let has_reader_downstream = succ[pi].iter().any(|&s| {
                self.passes[s]
                    .accesses
                    .iter()
                    .any(|a| writers[a.resource].contains(&pi))
            });
            if writes_something && !has_reader_downstream {
                sink[pi] = true;
            }
        }
        // Rückwärts über succ-Kanten: wer erreicht eine Senke?
        let mut pred: Vec<Vec<usize>> = vec![Vec::new(); self.passes.len()];
        for (i, ss) in succ.iter().enumerate() {
            for &s in ss {
                pred[s].push(i);
            }
        }
        let mut live = sink.clone();
        let mut stack: Vec<usize> = (0..self.passes.len()).filter(|&i| live[i]).collect();
        while let Some(i) = stack.pop() {
            for &p in &pred[i] {
                if !live[p] {
                    live[p] = true;
                    stack.push(p);
                }
            }
        }
        let order: Vec<usize> = order.into_iter().filter(|&i| live[i]).collect();

        // 7. Transitionen: pro Pass und Zugriff das Ziel-Layout; der
        //    Graph merkt sich das aktuelle Layout je Ressource und
        //    erzeugt eine Transition bei Wechsel.
        let mut current: Vec<Layout> =
            self.resources.iter().map(|r| r.initial).collect();
        let mut transitions = Vec::with_capacity(order.len());
        for &pi in &order {
            let mut tr = Vec::new();
            // Ziel-Layout je Ressource in diesem Pass (Write/ReadWrite
            // gewinnt gegen Read, falls derselbe Pass beides deklariert).
            let mut want: Vec<(usize, Layout)> = Vec::new();
            for a in &self.passes[pi].accesses {
                let target = a.kind.required_layout();
                match want.iter_mut().find(|(r, _)| *r == a.resource) {
                    Some(slot) => {
                        if a.kind != AccessKind::Read {
                            slot.1 = target;
                        }
                    }
                    None => want.push((a.resource, target)),
                }
            }
            for (r, target) in want {
                if current[r] != target {
                    tr.push((r, current[r], target));
                    current[r] = target;
                }
            }
            transitions.push(tr);
        }

        // 8. Lebensspannen.
        let mut spans = vec![None; self.resources.len()];
        for (o, &pi) in order.iter().enumerate() {
            for a in &self.passes[pi].accesses {
                let e = spans[a.resource].get_or_insert(ResourceSpan {
                    first: o,
                    last: o,
                    persistent: self.persistent.contains(&a.resource),
                });
                e.last = o;
            }
        }

        Ok(CompiledGraph { order, transitions, spans })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Klassische Deferred-Kette: G-Buffer → Lighting → Present.
    fn deferred() -> RenderGraph {
        let mut g = RenderGraph::new();
        let swap = g.add_resource("swapchain", Layout::Undefined);
        let gb = g.add_resource("gbuffer", Layout::Undefined);
        let lit = g.add_resource("lit", Layout::Undefined);
        g.add_pass("gbuffer", vec![RenderGraph::access(gb, AccessKind::Write)]);
        g.add_pass(
            "lighting",
            vec![
                RenderGraph::access(gb, AccessKind::Read),
                RenderGraph::access(lit, AccessKind::Write),
            ],
        );
        g.add_pass(
            "composite",
            vec![
                RenderGraph::access(lit, AccessKind::Read),
                RenderGraph::access(swap, AccessKind::Write),
            ],
        );
        g
    }

    /// Pass-Namen in Ausführungsreihenfolge.
    fn names(g: &RenderGraph, c: &CompiledGraph) -> Vec<String> {
        c.order.iter().map(|&i| g.passes()[i].name.clone()).collect()
    }

    #[test]
    fn kette_wird_in_reihenfolge_sortiert() {
        let g = deferred();
        let c = g.compile().unwrap();
        assert_eq!(names(&g, &c), ["gbuffer", "lighting", "composite"]);
    }

    #[test]
    fn transitionen_folgen_den_zustaenden() {
        let c = deferred().compile().unwrap();
        // Pass 0: gbuffer Undefined -> ColorAttachment
        assert_eq!(c.transitions[0], vec![(1, Layout::Undefined, Layout::ColorAttachment)]);
        // Pass 1: gbuffer muss lesbar werden, lit wird geschrieben
        assert!(c.transitions[1].contains(&(1, Layout::ColorAttachment, Layout::ShaderRead)));
        assert!(c.transitions[1].contains(&(2, Layout::Undefined, Layout::ColorAttachment)));
        // Pass 2: lit wird gelesen
        assert!(c.transitions[2].contains(&(2, Layout::ColorAttachment, Layout::ShaderRead)));
    }

    #[test]
    fn diamant_behaelt_topo_ordnung() {
        let mut g = RenderGraph::new();
        let a = g.add_resource("a", Layout::Undefined);
        let b = g.add_resource("b", Layout::Undefined);
        let c = g.add_resource("c", Layout::Undefined);
        let d = g.add_resource("d", Layout::Undefined);
        g.add_pass("root", vec![RenderGraph::access(a, AccessKind::Write)]);
        g.add_pass(
            "left",
            vec![
                RenderGraph::access(a, AccessKind::Read),
                RenderGraph::access(b, AccessKind::Write),
            ],
        );
        g.add_pass(
            "right",
            vec![
                RenderGraph::access(a, AccessKind::Read),
                RenderGraph::access(c, AccessKind::Write),
            ],
        );
        g.add_pass(
            "join",
            vec![
                RenderGraph::access(b, AccessKind::Read),
                RenderGraph::access(c, AccessKind::Read),
                RenderGraph::access(d, AccessKind::Write),
            ],
        );
        let comp = g.compile().unwrap();
        let pos = |n: &str| comp.order.iter().position(|&i| g.passes()[i].name == n).unwrap();
        assert!(pos("root") < pos("left"));
        assert!(pos("root") < pos("right"));
        assert!(pos("left") < pos("join"));
        assert!(pos("right") < pos("join"));
    }

    #[test]
    fn zyklus_wird_abgelehnt() {
        let mut g = RenderGraph::new();
        let x = g.add_resource("x", Layout::Undefined);
        let y = g.add_resource("y", Layout::Undefined);
        g.add_pass(
            "p",
            vec![
                RenderGraph::access(x, AccessKind::Write),
                RenderGraph::access(y, AccessKind::Read),
            ],
        );
        g.add_pass(
            "q",
            vec![
                RenderGraph::access(y, AccessKind::Write),
                RenderGraph::access(x, AccessKind::Read),
            ],
        );
        let e = g.compile().unwrap_err();
        match e {
            GraphError::Cycle { passes } => {
                assert_eq!(passes.len(), 2);
            }
            other => panic!("Zyklus erwartet, kam: {other:?}"),
        }
    }

    #[test]
    fn ungenutzter_pass_faellt_weg() {
        let mut g = deferred();
        // schreibt nur in eine Ressource, die niemand liest — aber sie
        // ist selbst Senke (Frame-Ende), also muss ein toter Pass einer
        // anderen Art her: schreibt in eine Ressource, die nur er selbst
        // in einem zweiten Pass liest, der wieder nichts liefert? Beides
        // wäre Senke. Echter Müll: ein Pass, der nur LIEST, was schon
        // gebraucht wird, und selbst nie gelesen wird — er ist keine
        // Senke (schreibt nichts) und fällt weg.
        let gb = 1; // "gbuffer" aus deferred()
        g.add_pass("guck_mal", vec![RenderGraph::access(gb, AccessKind::Read)]);
        let c = g.compile().unwrap();
        assert!(!c.order.iter().any(|&i| g.passes()[i].name == "guck_mal"));
        assert_eq!(c.order.len(), 3);
    }

    #[test]
    fn lesen_ohne_schreiber_ist_fehler() {
        let mut g = RenderGraph::new();
        let x = g.add_resource("x", Layout::Undefined);
        g.add_pass("gierig", vec![RenderGraph::access(x, AccessKind::Read)]);
        let e = g.compile().unwrap_err();
        assert_eq!(
            e,
            GraphError::UninitializedRead { pass: "gierig".into(), resource: "x".into() }
        );
    }

    #[test]
    fn persistente_history_darf_im_ersten_frame_gelesen_werden() {
        let mut g = RenderGraph::new();
        let cur = g.add_resource("current", Layout::Undefined);
        let hist = g.add_persistent("history", Layout::ShaderRead);
        let scene = g.add_pass("scene", vec![RenderGraph::access(cur, AccessKind::Write)]);
        // Explizite Abhaengigkeit: TAA liest current, schreibt es aber
        // nicht selbst — ohne depends_on waere die Reihenfolge nur
        // gluecklich, mit ihr garantiert.
        g.add_pass_with_deps(
            "taa",
            vec![
                RenderGraph::access(cur, AccessKind::Read),
                RenderGraph::access(hist, AccessKind::ReadWrite),
            ],
            vec![scene],
        );
        let c = g.compile().unwrap();
        let pos = |n: &str| c.order.iter().position(|&i| g.passes()[i].name == n).unwrap();
        assert!(pos("scene") < pos("taa"));
        assert_eq!(c.spans[hist].as_ref().unwrap().persistent, true);
    }

    #[test]
    fn explizite_abhaengigkeit_ordnet_ohne_gemeinsame_ressource() {
        let mut g = RenderGraph::new();
        let a = g.add_resource("a", Layout::Undefined);
        let b = g.add_resource("b", Layout::Undefined);
        let first = g.add_pass("zuerst", vec![RenderGraph::access(a, AccessKind::Write)]);
        // "spaeter" schreibt b, liest nichts — ohne Kante waere es
        // alphabetisch vor "zuerst" (rueckwaerts-Klammer als Tie-Break
        // faellt weg, weil b vor a alphabetisch kommt).
        g.add_pass_with_deps("spaeter", vec![RenderGraph::access(b, AccessKind::Write)], vec![first]);
        let c = g.compile().unwrap();
        assert_eq!(names(&g, &c), ["zuerst", "spaeter"]);
    }

    #[test]
    fn lebensspannen_zaehlen_von_bis() {
        let c = deferred().compile().unwrap();
        // gbuffer: geschrieben in Pass 0, gelesen in Pass 1 -> Span 0..1
        let s = c.spans[1].clone().unwrap();
        assert_eq!((s.first, s.last), (0, 1));
        // swapchain: nur Pass 2
        let s = c.spans[0].clone().unwrap();
        assert_eq!((s.first, s.last), (2, 2));
    }

    #[test]
    fn gleicher_input_gleichen_ablauf() {
        // Determinismus: zweimal kompilieren -> identischer Ablauf.
        let g = deferred();
        let a = g.compile().unwrap();
        let b = g.compile().unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn unbekannter_ressourcen_index_ist_fehler() {
        let mut g = RenderGraph::new();
        g.add_pass("spuk", vec![RenderGraph::access(42, AccessKind::Write)]);
        assert!(matches!(
            g.compile().unwrap_err(),
            GraphError::UnknownResource { pass, resource } if pass == "spuk" && resource == 42
        ));
    }
}
