# Hinweise zu Drittanbieter-Inhalten

(Übersetzung aus dem Englischen; verbindlich ist `THIRD_PARTY_NOTICES.md`.)

Code und Assets, die nicht aus diesem Repository stammen, mit den
Lizenzpflichten, die sie mitbringen. Normale Crate-Abhängigkeiten sind
durch `Cargo.lock` und die Upstream-Lizenztexte abgedeckt; diese Datei
nennt alles, was im Quellbaum selbst liegt.

## BC7-Decoder (`crates/re-io/src/bc7.rs`)

Bitlayout und Decode-Logik folgen der Referenzimplementierung
[bcdec](https://github.com/iOrange/bcdec) von Sergii Kudlai, portiert
nach Rust. Die Partitions-Tabellen für 2 und 3 Subsets stammen aus
bcdec, die sie aus der Microsoft-BC7-Spezifikation übernimmt.

bcdec ist dual lizenziert unter MIT / Unlicense; für diesen Port wurde
die MIT-Variante gewählt. Der MIT-Text (Copyright (c) 2022 Sergii
Kudlai) ist in der englischen Fassung dieser Datei zitiert; er gilt
wörtlich auch für diese Übersetzung.

## ETC2/EAC-Decoder (`crates/re-io/src/etc2.rs`)

Eigene Implementierung nach der OpenGL-ES-3.x-Spezifikation
(Anhang C: Blocklayouts, Modi, Modifier-Tabellen). Es wurde kein Code
übernommen; die gepackten Bitlayouts (inkl. der Redundanz-Kopien der
T-, H- und Planar-Modi) und der Modi-Diskriminierer wurden gegen drei
unabhängige Open-Source-Dekoder abgeglichen: AssetStudio (`etc.cpp`,
MIT), ANGLE (`loadimage_etc.cpp`, BSD-3-Clause) und SwiftShader
(`etcdec.cxx`, Apache-2.0).

## FBX-Test-Fixtures (`crates/re-io/tests/data/fbx/`)

Fünf binäre `.fbx`-Beispiele aus dem Regression-Katalog von
[ufbx](https://github.com/ufbx/ufbx) (`data/`): echte Exporte aus
Blender, Maya und 3ds Max als Import-Testfälle. ufbx und seine Daten
sind dual lizenziert (MIT / Unlicense); die Nutzung hier erfolgt unter
der MIT-Variante — der Lizenztext steht im englischen Dokument und in
`ATTRIBUTION.md` neben den Dateien.

## Inter-Schrift (`assets/fonts/inter/`)

Inter von Rasmus Andersson, lizenziert unter der SIL Open Font License
1.1. Der vollständige Lizenztext liegt neben den Schriftdateien
(`assets/fonts/inter/OFL.txt`).
