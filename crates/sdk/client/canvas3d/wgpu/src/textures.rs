//! Texture upload: a full mip chain built on the CPU (WebGL2 has no compute
//! and wgpu doesn't generate mips), downscaled to the device's limit.
//!
//! sRGB textures (base colour, emissive) are averaged in LINEAR light: box
//! filtering encoded values darkens every minified edge between light and
//! dark texels. Linear data (metallic-roughness, normals, occlusion) is
//! averaged as stored.

use canvas3d_core::{linear_to_srgb, srgb_to_linear, Texture};

/// One mip level: `width`×`height` RGBA8, tightly packed.
pub(crate) struct Level {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Build the mip chain for `tex`. Level 0 is the largest level whose
/// dimensions fit `max_dim` (halving the source as needed); the chain ends at
/// 1×1.
pub(crate) fn mip_chain(tex: &Texture, srgb: bool, max_dim: u32) -> Vec<Level> {
    let to_lin = decode_table(srgb);
    let mut level = Level { width: tex.width.max(1), height: tex.height.max(1), rgba: tex.rgba.clone() };
    while level.width > max_dim || level.height > max_dim {
        level = halve(&level, srgb, &to_lin);
    }
    let mut chain = vec![level];
    loop {
        let last = chain.last().expect("non-empty");
        if last.width == 1 && last.height == 1 {
            break;
        }
        let next = halve(last, srgb, &to_lin);
        chain.push(next);
    }
    chain
}

fn decode_table(srgb: bool) -> [f32; 256] {
    std::array::from_fn(|i| {
        let c = i as f32 / 255.0;
        if srgb {
            srgb_to_linear(c)
        } else {
            c
        }
    })
}

/// 2×2 box filter to half size (each dimension rounded down, at least 1; an
/// odd edge texel folds into its neighbour's average via clamped reads).
fn halve(src: &Level, srgb: bool, to_lin: &[f32; 256]) -> Level {
    let (w, h) = ((src.width / 2).max(1), (src.height / 2).max(1));
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    let at = |x: u32, y: u32, c: usize| {
        let x = x.min(src.width - 1);
        let y = y.min(src.height - 1);
        src.rgba[((y * src.width + x) * 4) as usize + c]
    };
    for y in 0..h {
        for x in 0..w {
            let (x0, y0) = (x * 2, y * 2);
            for c in 0..4 {
                let samples = [at(x0, y0, c), at(x0 + 1, y0, c), at(x0, y0 + 1, c), at(x0 + 1, y0 + 1, c)];
                let v = if srgb && c < 3 {
                    let lin = samples.iter().map(|&s| to_lin[s as usize]).sum::<f32>() * 0.25;
                    linear_to_srgb(lin)
                } else {
                    samples.iter().map(|&s| s as f32).sum::<f32>() * 0.25 / 255.0
                };
                out.push((v * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
            }
        }
    }
    Level { width: w, height: h, rgba: out }
}

/// Upload `tex` with its mip chain. `srgb` picks `Rgba8UnormSrgb` (sampling
/// decodes to linear) vs `Rgba8Unorm`.
pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &Texture,
    srgb: bool,
) -> wgpu::TextureView {
    let max_dim = device.limits().max_texture_dimension_2d;
    let chain = mip_chain(tex, srgb, max_dim);
    upload_levels(device, queue, &chain, srgb, "canvas3d-texture")
}

pub(crate) fn upload_levels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    chain: &[Level],
    srgb: bool,
    label: &str,
) -> wgpu::TextureView {
    let format = if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: chain[0].width, height: chain[0].height, depth_or_array_layers: 1 },
        mip_level_count: chain.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (i, level) in chain.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: i as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &level.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(level.width * 4),
                rows_per_image: Some(level.height),
            },
            wgpu::Extent3d { width: level.width, height: level.height, depth_or_array_layers: 1 },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A 1×1 texture of one colour (the default for an absent material slot).
pub(crate) fn solid(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4], srgb: bool) -> wgpu::TextureView {
    upload_levels(device, queue, &[Level { width: 1, height: 1, rgba: rgba.to_vec() }], srgb, "canvas3d-default")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn tex(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Arc<Texture> {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&f(x, y));
            }
        }
        Texture::from_rgba8(w, h, rgba)
    }

    #[test]
    fn chain_runs_to_one_by_one() {
        let t = tex(8, 2, |_, _| [10, 20, 30, 255]);
        let chain = mip_chain(&t, false, 4096);
        let dims: Vec<_> = chain.iter().map(|l| (l.width, l.height)).collect();
        assert_eq!(dims, vec![(8, 2), (4, 1), (2, 1), (1, 1)]);
        assert!(chain.iter().all(|l| l.rgba[..4] == [10, 20, 30, 255]));
    }

    #[test]
    fn oversized_textures_are_halved_to_fit_the_device() {
        let t = tex(16, 4, |_, _| [0, 0, 0, 255]);
        let chain = mip_chain(&t, false, 4);
        assert_eq!((chain[0].width, chain[0].height), (4, 1));
    }

    /// Black/white checker minified: the linear average of 0 and 1 is 0.5
    /// linear light = sRGB ~188, not the 128 a naive encoded average gives.
    #[test]
    fn srgb_mips_average_in_linear_light() {
        let t = tex(2, 2, |x, y| if (x + y) % 2 == 0 { [0, 0, 0, 255] } else { [255, 255, 255, 255] });
        let srgb = mip_chain(&t, true, 4096);
        let linear = mip_chain(&t, false, 4096);
        assert!((srgb[1].rgba[0] as i32 - 188).abs() <= 1, "{}", srgb[1].rgba[0]);
        assert!((linear[1].rgba[0] as i32 - 128).abs() <= 1, "{}", linear[1].rgba[0]);
        assert_eq!(srgb[1].rgba[3], 255, "alpha is averaged as data");
    }
}
