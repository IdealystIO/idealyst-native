//! Animated-image override textures — the renderer half of the stable-handle
//! scheme in [`encode`](crate::encode).
//!
//! An animated [`canvas_core::ImageSource`] (`generation > 0`, e.g. the video
//! frame pump) is encoded under ONE stable fake-blob [`ImageData`] per id; its
//! pixels arrive here as [`AnimUpload`]s. Each id gets one `wgpu::Texture`
//! that vello reads via `Renderer::override_image` — so the image's atlas slot
//! is allocated once and only its contents change per frame. Rebuilding the
//! Blob per frame instead makes vello's atlas (residency keyed by blob id)
//! grow/evict/repack at frame rate, which kills the WebGPU device within
//! seconds on web — silently.

use std::collections::HashMap;

use vello::peniko::ImageData;
use vello::Renderer;

use crate::encode::take_anim_uploads;

/// Per-render-loop registry of animated-image override textures, keyed by the
/// stable handle's blob id. Owned by each interactive renderer (native
/// `VelloCanvasRenderer`, web `WebRenderer`).
pub(crate) struct AnimTextures {
    map: HashMap<u64, (ImageData, wgpu::Texture)>,
}

impl AnimTextures {
    pub(crate) fn new() -> Self {
        Self { map: HashMap::new() }
    }

    /// Drain the encoder's pending animated-image frames: (re)create each id's
    /// override texture as needed, write the new pixels, and register + mark
    /// the override dirty on every given renderer. Call AFTER encoding the
    /// frame's scenes and BEFORE `render_to_texture`. Cheap when nothing is
    /// animated (one thread-local take of an empty queue).
    pub(crate) fn apply(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        main: &mut Renderer,
        image: Option<&mut Renderer>,
    ) {
        let (uploads, expired) = take_anim_uploads();
        if uploads.is_empty() && expired.is_empty() {
            return;
        }
        let mut renderers: Vec<&mut Renderer> = Vec::with_capacity(2);
        renderers.push(main);
        if let Some(r) = image {
            renderers.push(r);
        }
        for dead in expired {
            if self.map.remove(&dead.data.id()).is_some() {
                for r in renderers.iter_mut() {
                    r.override_image(&dead, None);
                }
            }
        }
        for up in uploads {
            let key = up.image.data.id();
            let needs_create = match self.map.get(&key) {
                Some((_, tex)) => tex.width() != up.width || tex.height() != up.height,
                None => true,
            };
            if needs_create {
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("canvas-vello-anim-image"),
                    size: wgpu::Extent3d {
                        width: up.width.max(1),
                        height: up.height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    // COPY_SRC: vello copies the texture into its atlas slot.
                    // COPY_DST: we write each decoded frame into it.
                    usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                self.map.insert(key, (up.image.clone(), tex));
            }
            let (_, tex) = self.map.get(&key).expect("just ensured");
            // Register on every renderer each changed frame — `override_image`
            // is an idempotent map insert + dirty mark, and doing it per change
            // (not per creation) also heals a lazily-created image renderer.
            for r in renderers.iter_mut() {
                r.override_image(
                    &up.image,
                    Some(wgpu::TexelCopyTextureInfoBase {
                        texture: tex.clone(),
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    }),
                );
            }
            if up.rgba.len() == (up.width as usize) * (up.height as usize) * 4 {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &up.rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * up.width),
                        rows_per_image: Some(up.height),
                    },
                    wgpu::Extent3d {
                        width: up.width,
                        height: up.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }
}
