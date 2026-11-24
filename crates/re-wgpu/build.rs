//! Bindet das Programm-Icon in Windows-Targets ein.
//!
//! `media/icons/icon.res` enthaelt RT_ICON (PNG-codiert, Vista+) und die
//! RT_GROUP_ICON-Verzeichnisstruktur. link.exe und rust-lld (COFF) nehmen
//! .res-Dateien als direkte Linker-Inputs an, daher brauchen wir dafuer
//! kein zusaetztes Crate. GNU-Targets (wuerden windres benoetigen) und
//! Nicht-Windows-Plattformen ueberspringen den Schritt — das
//! Fenster-Icon zur Laufzeit liefert unabhaengig davon re-platform aus
//! dem eingebetteten `icon-128.rgba`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    let res = manifest.join("../../media/icons/icon.res");
    println!("cargo:rerun-if-changed={}", res.display());

    let windows_msvc = std::env::var("CARGO_CFG_WINDOWS").is_ok()
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows_msvc && res.exists() {
        println!("cargo:rustc-link-arg-bin=re-demo={}", res.display());
    }
}
