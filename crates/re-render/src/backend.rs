//! Renderer-Anschluss der Engine: ein Trait, beliebig viele Backends.
//!
//! Der Kern (Scene-Traversal, Culling, Sortierung) haengt nur an diesem
//! Trait — GPU-APIs (wgpu/Vulkan/DX/WebGPU) und Konsolen-SDKs leben in
//! separaten Modulen/Crates. So bleibt RE-Code NDA-frei, waehrend
//! Konsolen-Backends privat implementiert werden koennen.

use glam::Mat4;
use re_core::material::Material;
use re_core::mesh::Mesh;
use thiserror::Error;

use crate::camera::Camera;

#[derive(Error, Debug)]
pub enum GfxError {
    #[error("kein geeignetes Geraet gefunden")]
    NoAdapter,
    #[error("Surface nicht verfuegbar: {0}")]
    Surface(String),
    #[error("Device/Queue-Fehler: {0}")]
    Device(String),
}

/// Billboard-Ausschnitt aus einer hochgeladenen Sheet-Textur.
///
/// Die Geometrie liefert das Quad (siehe [`crate::billboard`]), die
/// Textur kommt per [`GfxBackend::upload_texture`] unter einer frei
/// gewaehlten ID — z. B. ein Sprite-Sheet eines Konvertierers, dieses
/// Crate kennt aber keine Format-Details. `rect` ist das Pixelrechteck
/// (x, y, breite, hoehe) einer Zelle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    pub texture: u64,
    pub rect: [u32; 4],
}

/// Ein Draw-Batch: Geometrie + Material + Welttransform.
///
/// Batch-Granularitaet ist bewusst ein Mesh+Material-Paar; Instancing
/// (Tilemap-Zellen teilen sich ein Mesh) optimiert das Backend selbst.
/// Mit `Some(sprite)` wird das Mesh als Billboard-Ausschnitt unlit und
/// alpha-gemischt gezeichnet; material.base_color ist dann der Tint.
#[derive(Debug)]
pub struct DrawBatch<'a> {
    pub mesh: &'a Mesh,
    pub material: &'a Material,
    pub world: Mat4,
    /// Billboard-Ausschnitt; None = normale Mesh-Material-Pipeline.
    pub sprite: Option<Sprite>,
    /// Stream-Zellen-Id: ist sie gesetzt, zeichnet das Backend den
    /// Batch nur, wenn die Zelle ueber `GfxBackend::upload_cell`
    /// bereits in die GPU-Residenz uebernommen wurde (M3c-Handoff).
    /// None = klassische Route ueber das Mesh-Handle.
    pub cell: Option<u64>,
}

/// Lebenszyklus pro Frame: `init` einmal, dann `resize` bei Bedarf,
/// `render` + `present` pro Frame.
pub trait GfxBackend: Send {
    /// Stabil, menschenlesbar ("wgpu-vulkan", "software", …).
    fn name(&self) -> &'static str;

    /// Geraet/Pipelines anlegen; muss vor dem ersten `render` laufen.
    fn init(&mut self) -> Result<(), GfxError>;

    /// Groesse des Zeichenziels (in Pixeln) aendern.
    fn resize(&mut self, width: u32, height: u32);

    /// Eine Szene rendern. `batches` ist sortiert (opaque zuerst).
    fn render(&mut self, camera: &Camera, batches: &[DrawBatch]) -> Result<(), GfxError>;

    /// Sheet-Textur (RGBA8, dicht, 4 Byte/Pixel) unter `id` hochladen,
    /// damit Batches mit `sprite.texture == id` sie sampeln koennen.
    /// Backends ohne GPU duellen den No-op.
    fn upload_texture(&mut self, _id: u64, _rgba: &[u8], _width: u32, _height: u32) -> Result<(), GfxError> {
        Ok(())
    }

    /// Stream-Zelle in die GPU-Residenz uebernehmen (M3c-Handoff):
    /// ab jetzt zeichnen Batches mit `cell == Some(id)`. Der Aufruf
    /// kommt aus der Upload-Pipeline, nie aus dem Update-Thread.
    /// Backends ohne GPU-Residenz (Null) duellen den No-op und
    /// zeichnen Zell-Batches einfach mit.
    fn upload_cell(&mut self, _id: u64, _mesh: &Mesh) -> Result<(), GfxError> {
        Ok(())
    }

    /// GPU-Ressourcen einer Zelle freigeben. Kommt fruehestens
    /// nachdem der Handoff die Schonfrist ablaufen liess — niemals
    /// waehrend ein laufender Frame die Zelle noch zeichnet.
    fn release_cell(&mut self, _id: u64) {}

    /// Frame ausliefern; No-op fuer headless Backends.
    fn present(&mut self) -> Result<(), GfxError> {
        Ok(())
    }

    /// Fenster-Handle als Ziel setzen (M2, Window-Backends).
    ///
    /// Der Treiber (`re-platform`) reicht das Handle durch, das die App in
    /// `App::attach_window` bekommen hat — typischerweise ein
    /// `Arc<winit::window::Window>`, hier aber bewusst typgeloescht, damit
    /// dieses Crate kein winit kennt. Muss vor `init` aufgerufen werden;
    /// Backends ohne Fenster-Unterstuetzung liefern `Err(Surface(..))`.
    fn set_window_source(
        &mut self,
        _src: std::sync::Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), GfxError> {
        Err(GfxError::Surface("Backend unterstuetzt kein Fenster".into()))
    }
}

/// Headless-Backend fuer Tests, CI und Pi-Betrieb ohne Display:
/// zaehlt nur mit, was gerendert wuerde.
#[derive(Debug, Default)]
pub struct NullBackend {
    rendered_frames: u64,
    drawn_batches: u64,
    size: (u32, u32),
}

impl NullBackend {
    pub fn rendered_frames(&self) -> u64 {
        self.rendered_frames
    }
    pub fn drawn_batches(&self) -> u64 {
        self.drawn_batches
    }
    /// Aktuelle Zielgroesse in Pixeln (mind. 1x1).
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

impl GfxBackend for NullBackend {
    fn name(&self) -> &'static str {
        "null"
    }
    fn init(&mut self) -> Result<(), GfxError> {
        Ok(())
    }
    fn resize(&mut self, width: u32, height: u32) {
        self.size = (width.max(1), height.max(1));
    }
    fn render(&mut self, _camera: &Camera, batches: &[DrawBatch]) -> Result<(), GfxError> {
        self.rendered_frames += 1;
        self.drawn_batches += batches.len() as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_backend_zaehlt_batches() {
        let mut b = NullBackend::default();
        b.init().unwrap();
        b.resize(1280, 720);
        let mesh = Mesh::default();
        let mat = Material::default();
        let cam = Camera::perspective(
            glam::Vec3::new(0.0, 5.0, 10.0),
            glam::Vec3::ZERO,
            glam::Vec3::Y,
            1.0,
            16.0 / 9.0,
        );
        let batches = [
            DrawBatch { mesh: &mesh, material: &mat, world: Mat4::IDENTITY, sprite: None, cell: None },
            DrawBatch {
                mesh: &mesh,
                material: &mat,
                world: Mat4::from_translation(glam::Vec3::X),
                sprite: Some(Sprite { texture: 7, rect: [0, 0, 64, 64] }),
                cell: None,
            },
        ];
        b.render(&cam, &batches).unwrap();
        b.render(&cam, &batches).unwrap();
        assert_eq!(b.rendered_frames(), 2);
        assert_eq!(b.drawn_batches(), 4);
        // Default-Upload ist No-op (Headless-Backend ohne GPU).
        b.upload_texture(7, &[255; 4], 1, 1).unwrap();
    }
}
