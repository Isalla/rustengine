//! USD-ASCII-Import über die Endungs-Dispatch: `.usda`/`.usd` lesen,
//! binäre `.usdc` und Export sauber ablehnen. Die Parser-Semantik selbst
//! ist in `usd.rs` (Unit-Tests) abgedeckt.

use std::fs;
use std::path::PathBuf;

fn tmp(name: &str, content: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("re_usd_{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    path
}

#[test]
fn usda_über_endung_erkannt() {
    let p = tmp(
        "haus.usda",
        br#"#usda 1.0
def Xform "World"
{
    def Mesh "Tri"
    {
        point3f[] points = [(0,0,0), (1,0,0), (0,1,0)]
        int[] faceVertexIndices = [0, 1, 2]
    }
}
"#,
    );
    let proj = re_io::import(&p).expect(".usda sollte importieren");
    assert_eq!(proj.name, "haus");
    assert_eq!(proj.library.meshes.len(), 1);
}

#[test]
fn usd_mit_text_kopf_wird_ascii() {
    // `.usd` darf ASCII enthalten — am Inhalt entschieden, nicht an der Endung.
    let p = tmp("scene.usd", b"#usda 1.0\ndef Scope \"S\" { }\n");
    assert!(re_io::import(&p).is_ok());
}

#[test]
fn usdc_magie_meldet_binär() {
    let mut bytes = b"PXR-USDC".to_vec();
    bytes.extend_from_slice(&[0u8; 64]);
    let p = tmp("buero.usdc", &bytes);
    let e = re_io::import(&p).unwrap_err();
    let msg = format!("{e:#}");
    assert!(msg.contains("binär"), "klare Ansage: {msg}");
}

#[test]
fn export_bleibt_verboten() {
    let proj = re_core::project::Project::new("x");
    let p = PathBuf::from("ziel.usda");
    let e = re_io::export(&proj, &p).unwrap_err();
    assert!(e.to_string().contains("nur zum Importieren"), "klare Ansage: {e}");
}
