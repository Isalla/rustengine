# Ein Prozess, ein VRAM-Budget

[English](vram.md) | Deutsch

## Warum alles in EINEM Programm läuft (Ausgangsproblem)

Typische Arbeitsumgebung: 3D-Editor + externer Bild→Mesh-Generator
(ComfyUI/Tripo/3D-Gen-Studio) als separate Programme. Auf knappen
Konsumenten-Karten (12 GB) konkurrieren beide um denselben VRAM — jeder hält seine Reserven
dauerhaft belegt, der Editor stürzt mit „VRAM zu gering" ab, obwohl gerade
nur eines der Programme wirklich rechnet.

Lösung RE: **Engine, Editor, KI-Anbindung und Generierung teilen sich einen
Prozess und damit einen einzigen VRAM-Haushalt.** Nur ein Programm kann
entscheiden, *wann* welche Daten die Grafikkarte verlassen dürfen:

* Der Generator (Diffusion/DiT-Gewichte, 4–8 GB) ist **kein dauerhaft
  residenter Dienst**, sondern ein Modul, das bei Bedarf geladen und nach
  dem Job wieder **aus dem VRAM geworfen** wird.
* Editor-Reserven (Textur-MipLevels, Backups, Bake-Zwischenpuffer) sind
  **zurückholbar**: RAM-Kopie bleibt, GPU-Kopie wird vernichtet und bei
  Bedarf neu hochgeladen.
* Die KI-Inferenz (lokal: Ollama/llama.cpp oder eingebettete Runner) meldet
  ihren Bedarf demselben Budget.

Externe Dienst-Prozesse bleiben möglich (ComfyUI auf zweiter GPU, Cloud-API) —
aber sie sind Option, nicht Grundarchitektur: was im selben Prozess laufen
kann, läuft im selben Prozess.

## Budget-Manager (Design, M5/M6)

In `re-render` (Backend-unabhängig, trägt `re-wgpu`/Vulkan aus):

```
VramBudget
  ├─ total_bytes            (per Device-Limit/Query, beschreibbar konfigurierbar)
  ├─ reservations           ( Modul → Reserve: Editor | Gen | Inference | Headroom )
  └─ tracked resources      (jede Textur/Buffer mit: Bytegröße, Priorität,
                             RAM-Mirror ja/nein, zuletzt benutzt)
```

Regeln:

1. **Jede GPU-Ressource ist registriert** — Zuweisung läuft nur noch über den
   Manager (`create_texture_tracked(desc, prio, mirror)`). Ohne Mirror keine
   Evakuierung.
2. **Evakuierung ist sichtbar, nicht magisch:** Bei Budget-Engpass sortiert
   der Manager nach Priorität × Alter und *zerstört* GPU-Kopien
   (`texture = None`, RAM-Daten bleiben). Beim nächsten Zugriff lädt der
   normale Upload-Pfad neu. Sichtbares Symptom im Editor: ein Frame Flackern,
   nie ein Crash.
3. **Gen-Läufe reservieren im Voraus:** `Gen.begin(bytes)` evakuiert so lange
   Editor-Material, bis die Reserve frei meldbar ist; schlägt das fehl
   (weil alles hoch-prioritär ist), kommt eine *Fehlermeldung mit Zahlenseite*
   („brauche 6.2 GiB, frei 3.1 GiB — was darf weichen?"), kein OOM-Absturz.
   `Gen.end()` gibt frei, Editor-Material kommt beim nächsten Zugriff
   automatisch (lazy) zurück.
4. **Prioritäten** (fest, dokumentiert): laufende Render-Ressourcen des
   Editors = hoch; MipLevels > Anzeigebedarf, Bake-Zwischenpuffer,
   Vorschau-Caches = mittel; Gen-Gewichte = hoch *während* eines Jobs, sonst
   0 (RAM/Platte); Inferenz-KV-Cache = hoch während Antwort.
5. **Web-Ziel:** Der Browser verwaltet VRAM selbst; dort ist der Manager nur
   Budget-Zähler + Verursacher-Log (kein Evakuieren), damit sich Verhalten
   zwischen Plattformen nicht grundlegend unterscheidet.

## Konsequenzen für bestehende Zusagen

* `docs/ki-integration.de.md` §2: `GenService` bleibt das **Interface**, aber
  die empfohlene Implementierung ist **in-process** (offene Modelle, z. B.
  burn/candle-Runner für Bild→Mesh/Bild→Bild). HTTP-/Cloud-Adapter bleiben
  als `GenService`-Varianten für zweite GPUs und große Modelle.
* Ressourcen-Erzeugung in `re-wgpu` bekommt den Tracking-Hook, bevor große
  Szenarien (Karten-Texturen) gebaut werden — sonst kann der Manager später
  nichts evakuieren, was er nie gesehen hat.
* Der Cinema-Editor (M7) ist mitgemeint: Vorschau-Renderer fahren ihr
  Budget runter, wenn ein Gen-Job anläuft (gleicher Prozess, gleiche Regeln).
