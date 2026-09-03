//! Rendering primitives: single-chunk PNG output, multi-chunk compositing,
//! scaled blitting, supersampled / hypersampled blitting, and the whole-map
//! shadow + ambient-occlusion passes.
//!
//! The color resolution rules live in the `color` module and the shadow /
//! supersampling / ambient-occlusion math lives in the `light` module; this
//! module turns them into pixels.

use std::collections::HashMap;
use std::path::Path;

use image::{Rgb, RgbImage};

use crate::chunk::{ChunkTop, COLUMNS, VOID_H, CHUNK_SIZE};
use crate::color::{apply_ao, display_color, light_bloom_color, night_darken, night_shade, shade, NO_BLOCK};
use crate::light::{ambient_occlusion, bloom, BLOOM_RADIUS_BLOCKS, NIGHT_BLOOM_RADIUS_BLOCKS};

/// Render the top-down view of a chunk as a PNG.
///
/// At `scale = 1` the output is 16x16 (1 pixel = 1 block) and each pixel is
/// colored by the highest non-air block in that column. At `scale = N` the
/// output is `ceil(16 / N)` on each side, and every pixel is colored by the
/// most common block in its `N x N` area.
pub(crate) fn render_chunk_png(
    top: &ChunkTop,
    out_path: &Path,
    scale: u32,
    transparency: bool,
    night: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let size = (CHUNK_SIZE + scale as usize - 1) / scale as usize;

    let mut img = RgbImage::new(size as u32, size as u32);

    blit_scaled(
        &mut img,
        &top.blocks,
        &top.under,
        CHUNK_SIZE,
        CHUNK_SIZE,
        scale,
        0,
        0,
        transparency,
        night,
    );

    img.save(out_path)?;

    Ok(())
}

/// Blit a chunk into an already-sized shared image at a given pixel offset.
///
/// `offset_x` / `offset_z` are the pixel coordinates of the chunk's top-left
/// corner inside the big image. At `scale = 1` that is the chunk's block
/// offset; at `scale = N` it is the chunk's block offset divided by N, and the
/// chunk is downsampled so each pixel represents an `N x N` block area.
pub(crate) fn render_into_big(
    img: &mut RgbImage,
    top: &ChunkTop,
    offset_x: u32,
    offset_z: u32,
    scale: u32,
    transparency: bool,
    night: bool,
) {
    blit_scaled(
        img,
        &top.blocks,
        &top.under,
        CHUNK_SIZE,
        CHUNK_SIZE,
        scale,
        offset_x,
        offset_z,
        transparency,
        night,
    );
}

/// Write a downsampled top-down view of a rectangular block region into
/// `out`, starting at pixel (`origin_x`, `origin_z`).
///
/// `blocks` is a row-major grid of `region_w x region_h` columns (index =
/// `z * region_w + x`), where each entry is the highest non-air block in that
/// column, or `None` for void.
///
/// At `scale = 1` each output pixel is a single block. At `scale = N` each
/// output pixel covers an `N x N` block area and is colored by the most common
/// block in that area (void is ignored; an all-void area is black).
fn blit_scaled(
    out: &mut RgbImage,
    blocks: &[Option<String>],
    under: &[Option<String>],
    region_w: usize,
    region_h: usize,
    scale: u32,
    origin_x: u32,
    origin_z: u32,
    transparency: bool,
    night: bool,
) {
    let scale = scale.max(1) as usize;

    // Fast path: no downsampling, one pixel per block.
    if scale == 1 {
        for z in 0..region_h {
            for x in 0..region_w {
                let i = z * region_w + x;
                let mut rgb = display_color(&blocks[i], &under[i], transparency);
                if night {
                    rgb = night_darken(rgb);
                }
                out.put_pixel(origin_x + x as u32, origin_z + z as u32, Rgb(rgb));
            }
        }
        return;
    }

    let cells_w = (region_w + scale - 1) / scale;
    let cells_h = (region_h + scale - 1) / scale;

    for cz in 0..cells_h {
        for cx in 0..cells_w {
            let x0 = cx * scale;
            let z0 = cz * scale;
            let x1 = (x0 + scale).min(region_w);
            let z1 = (z0 + scale).min(region_h);

            let mut rgb = most_common_color(blocks, under, region_w, x0, x1, z0, z1, transparency);
            if night {
                rgb = night_darken(rgb);
            }

            out.put_pixel(origin_x + cx as u32, origin_z + cz as u32, Rgb(rgb));
        }
    }
}


/// Write a top-down view where every block is a solid `s x s` pixel square,
/// starting at pixel (`origin_x`, `origin_z`).
///
/// `blocks` is a row-major `region_w x region_h` grid of the highest non-air
/// block per column (index = `z * region_w + x`), or `None` for void. Unlike
/// [`blit_scaled`], this upsamples without any interpolation: every pixel of a
/// block's `s x s` square gets the block's own color (void is [`NO_BLOCK`]).
pub(crate) fn blit_supersampled(
    out: &mut RgbImage,
    blocks: &[Option<String>],
    under: &[Option<String>],
    region_w: usize,
    region_h: usize,
    s: usize,
    origin_x: u32,
    origin_z: u32,
    transparency: bool,
    night: bool,
) {
    let s = s.max(1);
    for z in 0..region_h {
        for x in 0..region_w {
            let i = z * region_w + x;
            let mut rgb = display_color(&blocks[i], &under[i], transparency);
            if night {
                rgb = night_darken(rgb);
            }
            let base_x = origin_x + (x as u32) * (s as u32);
            let base_z = origin_z + (z as u32) * (s as u32);
            for dz in 0..s as u32 {
                for dx in 0..s as u32 {
                    out.put_pixel(base_x + dx, base_z + dz, Rgb(rgb));
                }
            }
        }
    }
}

/// Render the top-down view of a chunk as a supersampled PNG: each of the
/// 16x16 blocks becomes a solid `scale x scale` pixel square, so the output is
/// `16 * scale` on each side (no interpolation between blocks).
pub(crate) fn render_chunk_png_ss(
    top: &ChunkTop,
    out_path: &Path,
    scale: u32,
    transparency: bool,
    night: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let size = CHUNK_SIZE * scale as usize;
    let mut img = RgbImage::new(size as u32, size as u32);
    blit_supersampled(
        &mut img,
        &top.blocks,
        &top.under,
        CHUNK_SIZE,
        CHUNK_SIZE,
        scale as usize,
        0,
        0,
        transparency,
        night,
    );
    img.save(out_path)?;

    Ok(())
}

/// Render the top-down view of a chunk as a supersampled PNG, optionally
/// layering ambient occlusion and/or light bloom on top of the solid
/// `scale x scale` block squares (as in [`render_chunk_png_ss`]).
///
/// `with_ao` darkens each block's edge facing a neighbour one block higher
/// (see [`ambient_occlusion`]); `with_bloom` adds a radial gradient of light
/// around light-emitting blocks (see [`bloom`]). Both are computed from the
/// chunk's own columns, so the effects are correct within the chunk but the
/// outermost edge blocks cannot see neighbours that belong to other chunks.
pub(crate) fn render_chunk_png_ss_fx(
    top: &ChunkTop,
    out_path: &Path,
    scale: u32,
    transparency: bool,
    with_ao: bool,
    with_bloom: bool,
    night: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let s = scale as usize;
    let size = CHUNK_SIZE * s;
    let mut img = RgbImage::new(size as u32, size as u32);

    let ao = if with_ao {
        // Reuse the global AO routine on this chunk's 16x16 height grid (void
        // columns keep the VOID_H sentinel so they neither cast nor receive).
        let heights: Vec<i32> =
            top.heights.iter().map(|h| h.unwrap_or(VOID_H)).collect();
        Some(ambient_occlusion(&heights, CHUNK_SIZE, CHUNK_SIZE, s))
    } else {
        None
    };

    let bloom_field = if with_bloom {
        // This chunk's per-block bloom colors (void / non-light -> black).
        let lights: Vec<[u8; 3]> = top
            .blocks
            .iter()
            .map(|b| b.as_deref().and_then(light_bloom_color).unwrap_or(NO_BLOCK))
            .collect();
        let radius = if night { NIGHT_BLOOM_RADIUS_BLOCKS } else { BLOOM_RADIUS_BLOCKS };
        Some(bloom(&lights, CHUNK_SIZE, CHUNK_SIZE, s, radius))
    } else {
        None
    };

    blit_supersampled_fx(
        &mut img,
        &top.blocks,
        &top.under,
        CHUNK_SIZE,
        CHUNK_SIZE,
        s,
        ao.as_deref(),
        bloom_field.as_deref(),
        0,
        0,
        transparency,
        night,
    );

    img.save(out_path)?;

    Ok(())
}

/// Like [`blit_supersampled`], but each block's `s x s` pixels are additionally
/// darkened toward black by an ambient-occlusion field and/or brightened by an
/// additive light-bloom field.
///
/// `ao`, when present, is a per-pixel darkening amount (0.0..=1.0); `bloom`,
/// when present, is a per-pixel additive RGB amount. Both are sized
/// `region_w * s x region_h * s` (index = `z * region_w * s + x * s +` pixel
/// offset). AO darkens first, then bloom brightens, so a shadowed pixel next
/// to a light still glows.
fn blit_supersampled_fx(
    out: &mut RgbImage,
    blocks: &[Option<String>],
    under: &[Option<String>],
    region_w: usize,
    region_h: usize,
    s: usize,
    ao: Option<&[f32]>,
    bloom: Option<&[[u8; 3]]>,
    origin_x: u32,
    origin_z: u32,
    transparency: bool,
    night: bool,
) {
    let s = s.max(1);
    let pw = region_w * s;
    for z in 0..region_h {
        for x in 0..region_w {
            let i = z * region_w + x;
            let mut rgb = display_color(&blocks[i], &under[i], transparency);
            if night {
                rgb = night_darken(rgb);
            }
            let base_x = origin_x + (x as u32) * (s as u32);
            let base_z = origin_z + (z as u32) * (s as u32);
            for dz in 0..s as u32 {
                for dx in 0..s as u32 {
                    let px = base_x + dx;
                    let pz = base_z + dz;
                    let pi = (pz as usize) * pw + (px as usize);
                    let mut rgb = rgb;
                    if let Some(ao) = ao {
                        let a = ao[pi];
                        if a > 0.0 {
                            rgb = apply_ao(rgb, a);
                        }
                    }
                    if let Some(bloom) = bloom {
                        let b = bloom[pi];
                        if b != [0, 0, 0] {
                            rgb = [
                                rgb[0].saturating_add(b[0]),
                                rgb[1].saturating_add(b[1]),
                                rgb[2].saturating_add(b[2]),
                            ];
                        }
                    }
                    out.put_pixel(px, pz, Rgb(rgb));
                }
            }
        }
    }
}

/// Color of the most common block within the block rectangle
/// `[x0, x1) x [z0, z1)`.
///
/// Each column is resolved to its display color (see [`display_color`], which
/// applies water transparency when enabled), and the most common color wins.
/// Ties resolve to whichever color reaches the leading count first in
/// row-major scan order, so the result is deterministic. Void columns are
/// ignored; an all-void area returns `NO_BLOCK`.
fn most_common_color(
    blocks: &[Option<String>],
    under: &[Option<String>],
    region_w: usize,
    x0: usize,
    x1: usize,
    z0: usize,
    z1: usize,
    transparency: bool,
) -> [u8; 3] {
    let mut counts: HashMap<[u8; 3], u32> = HashMap::new();
    let mut best: Option<[u8; 3]> = None;
    let mut best_count: u32 = 0;

    for z in z0..z1 {
        for x in x0..x1 {
            let i = z * region_w + x;
            let rgb = display_color(&blocks[i], &under[i], transparency);
            if rgb == NO_BLOCK {
                continue;
            }

            let count = counts.entry(rgb).or_insert(0);
            *count += 1;

            if *count > best_count {
                best_count = *count;
                best = Some(rgb);
            }
        }
    }

    best.unwrap_or(NO_BLOCK)
}

/// Final shadow pass: fill `img` from the global `colors` + `shadow` grids,
/// darkening shadowed columns.
///
/// Every chunk in the world bounding box is blitted at the same pixel offset
/// the non-shadow single-mode pass uses, so the two modes share an identical
/// layout. Chunks that do not exist are all void in the grids and therefore
/// blit as black -- exactly the image's initial state.
pub(crate) fn render_shaded_map(
    img: &mut RgbImage,
    colors: &[[u8; 3]],
    shadow: &[bool],
    grid_w: usize,
    min_chunk_x: i32,
    min_chunk_z: i32,
    max_chunk_x: i32,
    max_chunk_z: i32,
    scale: u32,
    night: bool,
) {
    let shade_fn = if night { night_shade } else { shade };
    for cz in min_chunk_z..=max_chunk_z {
        for cx in min_chunk_x..=max_chunk_x {
            let block_x0 = (cx - min_chunk_x) * (CHUNK_SIZE as i32);
            let block_z0 = (cz - min_chunk_z) * (CHUNK_SIZE as i32);

            // This chunk's 16x16 display colors, with shadow already applied.
            let mut local: Vec<[u8; 3]> = Vec::with_capacity(COLUMNS);
            for lz in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let gx = block_x0 as usize + lx;
                    let gz = block_z0 as usize + lz;
                    let gi = gz * grid_w + gx;
                    let c = colors[gi];
                    local.push(if shadow[gi] { shade_fn(c) } else { c });
                }
            }

            let offset_x = (block_x0 / scale as i32) as u32;
            let offset_z = (block_z0 / scale as i32) as u32;
            blit_scaled_shaded(
                img,
                &local,
                CHUNK_SIZE,
                CHUNK_SIZE,
                scale,
                offset_x,
                offset_z,
            );
        }
    }
}

/// General supersampled fill: draw every block as a solid `s x s` pixel square
/// of its base color (no interpolation between blocks), optionally darkening
/// each pixel independently by an elevation shadow (`bool`) and/or an
/// ambient-occlusion amount (`f32`, 0.0 = no darkening, 1.0 = fully black),
/// and/or brightening it additively by a light-bloom color (`[u8; 3]`).
///
/// When `pixel_shadow` is provided it is a pixel-resolution mask (see
/// [`supersampled_heights`]), so a block straddling the shadow edge gets a mix
/// of lit and dark pixels and the diagonal shadow boundary stays smooth. When
/// `pixel_ao` is provided it is a per-pixel darkening amount and
/// `pixel_bloom` a per-pixel additive RGB amount. All masks, when present, are
/// sized `grid_w * s x grid_h * s` (index = `z * grid_w * s + x * s +` pixel
/// offset). Shadow and AO darken first, then bloom brightens, so the three
/// compose.
pub(crate) fn render_ss(
    img: &mut RgbImage,
    colors: &[[u8; 3]],
    pixel_shadow: Option<&[bool]>,
    pixel_ao: Option<&[f32]>,
    pixel_bloom: Option<&[[u8; 3]]>,
    grid_w: usize,
    grid_h: usize,
    s: usize,
    night: bool,
) {
    let pw = grid_w * s;
    let shade_fn = if night { night_shade } else { shade };

    for z in 0..grid_h {
        for x in 0..grid_w {
            let c = colors[z * grid_w + x];
            let base_x = x * s;
            let base_z = z * s;

            for dz in 0..s {
                for dx in 0..s {
                    let px = base_x + dx;
                    let pz = base_z + dz;
                    let pi = pz * pw + px;
                    let mut rgb = c;
                    if let Some(shadow) = pixel_shadow {
                        if shadow[pi] {
                            rgb = shade_fn(rgb);
                        }
                    }
                    if let Some(ao) = pixel_ao {
                        let a = ao[pi];
                        if a > 0.0 {
                            rgb = apply_ao(rgb, a);
                        }
                    }
                    if let Some(bloom) = pixel_bloom {
                        let b = bloom[pi];
                        if b != [0, 0, 0] {
                            rgb = [
                                rgb[0].saturating_add(b[0]),
                                rgb[1].saturating_add(b[1]),
                                rgb[2].saturating_add(b[2]),
                            ];
                        }
                    }
                    img.put_pixel(px as u32, pz as u32, Rgb(rgb));
                }
            }
        }
    }
}

/// Write a chunk's per-block display colors into `out`, starting at pixel
/// (`origin_x`, `origin_z`), mirroring [`blit_scaled`]'s layout.
///
/// `colors` is a row-major `region_w x region_h` grid of display colors where
/// a void column is already stored as [`NO_BLOCK`]. At `scale = 1` each output
/// pixel is one block; at `scale = N` each pixel is the most common non-void
/// display color in its `N x N` block area.
fn blit_scaled_shaded(
    out: &mut RgbImage,
    colors: &[[u8; 3]],
    region_w: usize,
    region_h: usize,
    scale: u32,
    origin_x: u32,
    origin_z: u32,
) {
    let scale = scale.max(1) as usize;

    if scale == 1 {
        for z in 0..region_h {
            for x in 0..region_w {
                let rgb = colors[z * region_w + x];
                out.put_pixel(origin_x + x as u32, origin_z + z as u32, Rgb(rgb));
            }
        }
        return;
    }

    let cells_w = (region_w + scale - 1) / scale;
    let cells_h = (region_h + scale - 1) / scale;

    for cz in 0..cells_h {
        for cx in 0..cells_w {
            let x0 = cx * scale;
            let z0 = cz * scale;
            let x1 = (x0 + scale).min(region_w);
            let z1 = (z0 + scale).min(region_h);

            let rgb = most_common_shaded_color(colors, region_w, x0, x1, z0, z1);
            out.put_pixel(origin_x + cx as u32, origin_z + cz as u32, Rgb(rgb));
        }
    }
}

/// Display color of the most common (non-void) block in the block rectangle
/// `[x0, x1) x [z0, z1)`, mirroring [`most_common_color`]'s tie-breaking.
///
/// [`NO_BLOCK`] (void) entries are ignored; an all-void area returns
/// [`NO_BLOCK`].
fn most_common_shaded_color(
    colors: &[[u8; 3]],
    region_w: usize,
    x0: usize,
    x1: usize,
    z0: usize,
    z1: usize,
) -> [u8; 3] {
    let mut counts: HashMap<[u8; 3], u32> = HashMap::new();
    let mut best: Option<[u8; 3]> = None;
    let mut best_count: u32 = 0;

    for z in z0..z1 {
        for x in x0..x1 {
            let rgb = colors[z * region_w + x];
            if rgb == NO_BLOCK {
                continue;
            }

            let count = counts.entry(rgb).or_insert(0);
            *count += 1;

            if *count > best_count {
                best_count = *count;
                best = Some(rgb);
            }
        }
    }

    best.unwrap_or(NO_BLOCK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{HYPER_SAMPLE, SUPER_SAMPLE};
    use crate::palette::block_color;
    use crate::light::{ambient_occlusion, bloom, compute_shadows, supersampled_heights};
    use image::GenericImageView;

    #[test]
    fn renders_a_16x16_png_with_expected_pixels() {
        // Only column (x=0, z=0) has a block; everything else is void.
        let blocks = std::array::from_fn(|i| {
            (i == 0).then(|| "minecraft:grass_block".to_string())
        });
        let top = ChunkTop {
            blocks,
            heights: [None; COLUMNS],
            under: std::array::from_fn(|_| None),
        };

        let path = std::env::temp_dir().join("worldraw_test_chunk.png");
        render_chunk_png(&top, &path, 1, false, false).expect("render should succeed");

        let img = image::open(&path).expect("png should be readable");
        assert_eq!(img.dimensions(), (16, 16));

        // Top-left pixel is the grass column.
        assert_eq!(img.get_pixel(0, 0).0, [106, 170, 64, 255]);
        // Every other column is void (black).
        assert_eq!(img.get_pixel(1, 0).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(15, 15).0, [0, 0, 0, 255]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn composites_chunks_into_a_single_image() {
        // Chunk A occupies pixel columns 0..16 and only column (x=0, z=0) is
        // grass. Chunk B occupies pixel columns 16..32 and is all sand.
        let top_a = ChunkTop {
            blocks: std::array::from_fn(|i| {
                (i == 0).then(|| "minecraft:grass_block".to_string())
            }),
            heights: [None; COLUMNS],
            under: std::array::from_fn(|_| None),
        };
        let top_b = ChunkTop {
            blocks: std::array::from_fn(|_| Some("minecraft:sand".to_string())),
            heights: [None; COLUMNS],
            under: std::array::from_fn(|_| None),
        };

        let mut img = RgbImage::new(32, 16);
        render_into_big(&mut img, &top_a, 0, 0, 1, false, false);
        render_into_big(&mut img, &top_b, 16, 0, 1, false, false);

        assert_eq!(img.dimensions(), (32, 16));

        // Chunk A: top-left pixel is the grass column.
        assert_eq!(img.get_pixel(0, 0).0, [106, 170, 64]);
        // The rest of chunk A is void (black).
        assert_eq!(img.get_pixel(1, 0).0, [0, 0, 0]);
        assert_eq!(img.get_pixel(15, 15).0, [0, 0, 0]);

        // Chunk B starts at x=16 and is entirely sand.
        assert_eq!(img.get_pixel(16, 0).0, [219, 209, 160]);
        assert_eq!(img.get_pixel(31, 15).0, [219, 209, 160]);
    }

    #[test]
    fn downsamples_by_most_common_block() {
        // Top-left 2x2 area: three sand + one grass; everything else is void.
        let mut blocks: [Option<String>; COLUMNS] =
            std::array::from_fn(|_| None);
        blocks[0] = Some("minecraft:sand".to_string());
        blocks[1] = Some("minecraft:sand".to_string());
        blocks[16] = Some("minecraft:sand".to_string());
        blocks[17] = Some("minecraft:grass_block".to_string());

        let top = ChunkTop {
            blocks,
            heights: [None; COLUMNS],
            under: std::array::from_fn(|_| None),
        };

        let path = std::env::temp_dir().join("worldraw_test_scaled.png");
        render_chunk_png(&top, &path, 2, false, false).expect("render should succeed");

        let img = image::open(&path).expect("png should be readable");
        assert_eq!(img.dimensions(), (8, 8));

        // Cell (0, 0) covers the 2x2 area; sand (3) beats grass (1).
        assert_eq!(img.get_pixel(0, 0).0, [219, 209, 160, 255]);
        // Every other cell is void (black).
        assert_eq!(img.get_pixel(1, 0).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(0, 1).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(7, 7).0, [0, 0, 0, 255]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scale_four_shrinks_a_16x16_chunk_to_4x4() {
        // Uniform grass -> the modal block of every 4x4 area is grass.
        let top = ChunkTop {
            blocks: std::array::from_fn(|_| {
                Some("minecraft:grass_block".to_string())
            }),
            heights: [None; COLUMNS],
            under: std::array::from_fn(|_| None),
        };

        let path = std::env::temp_dir().join("worldraw_test_scaled4.png");
        render_chunk_png(&top, &path, 4, false, false).expect("render should succeed");

        let img = image::open(&path).expect("png should be readable");
        assert_eq!(img.dimensions(), (4, 4));

        for z in 0..4 {
            for x in 0..4 {
                assert_eq!(img.get_pixel(x, z).0, [106, 170, 64, 255]);
            }
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn shaded_map_renders_darkened_shadow_cells() {
        let grid_w = 16;
        let grid_h = 16;
        // A single chunk of flat sand field at height 0 with one pillar of
        // height 3 at (x=4, z=1).
        let mut heights = vec![0i32; grid_w * grid_h];
        heights[1 * grid_w + 4] = 3;
        let colors: Vec<[u8; 3]> =
            vec![block_color("minecraft:sand"); grid_w * grid_h];
        let shadow = compute_shadows(&heights, grid_w, grid_h);

        let mut img = RgbImage::new(16, 16);
        render_shaded_map(&mut img, &colors, &shadow, grid_w, 0, 0, 0, 0, 1, false);

        let lit = block_color("minecraft:sand");
        let dark = shade(lit);

        // The three diagonal cells toward the bottom-left are shadowed.
        assert_eq!(img.get_pixel(3, 2).0, dark);
        assert_eq!(img.get_pixel(2, 3).0, dark);
        assert_eq!(img.get_pixel(1, 4).0, dark);
        // The pillar top and the cell just past the shadow stay at the lit
        // color, as does the rest of the flat field.
        assert_eq!(img.get_pixel(4, 1).0, lit);
        assert_eq!(img.get_pixel(0, 5).0, lit);
        assert_eq!(img.get_pixel(0, 0).0, lit);
    }

    #[test]
    fn supersample_renders_solid_5x5_blocks_without_interpolation() {
        // Only block (x=0, z=0) is grass; everything else is void.
        let top = ChunkTop {
            blocks: std::array::from_fn(|i| {
                (i == 0).then(|| "minecraft:grass_block".to_string())
            }),
            heights: [None; COLUMNS],
            under: std::array::from_fn(|_| None),
        };

        let path = std::env::temp_dir().join("worldraw_test_ss.png");
        render_chunk_png_ss(&top, &path, SUPER_SAMPLE, false, false).expect("render should succeed");

        let img = image::open(&path).expect("png should be readable");
        assert_eq!(img.dimensions(), (16 * 5, 16 * 5));

        // The grass block at (0,0) is a solid 5x5 grass square.
        for z in 0..5u32 {
            for x in 0..5u32 {
                assert_eq!(img.get_pixel(x, z).0, [106, 170, 64, 255]);
            }
        }
        // The neighbouring block (1,0) is void -> black.
        assert_eq!(img.get_pixel(5, 0).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(79, 79).0, [0, 0, 0, 255]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn supersample_shaded_map_draws_solid_blocks_with_smooth_shadow() {
        let grid_w = 8;
        let grid_h = 8;
        let s = 5usize;
        let mut heights = vec![0i32; grid_w * grid_h];
        heights[1 * grid_w + 4] = 3;
        let colors: Vec<[u8; 3]> =
            vec![block_color("minecraft:sand"); grid_w * grid_h];

        let pixel_heights = supersampled_heights(&heights, grid_w, grid_h, s);
        let pw = grid_w * s;
        let ph = grid_h * s;
        let pixel_shadow = compute_shadows(&pixel_heights, pw, ph);

        let mut img = RgbImage::new((grid_w * s) as u32, (grid_h * s) as u32);
        render_ss(&mut img, &colors, Some(&pixel_shadow), None, None, grid_w, grid_h, s, false);

        let lit = block_color("minecraft:sand");
        let dark = shade(lit);

        // Block (0,0) is far from any shadow: all its 5x5 pixels are lit.
        for z in 0..s {
            for x in 0..s {
                assert_eq!(img.get_pixel(x as u32, z as u32).0, lit);
            }
        }

        // Some block straddles the shadow edge, so within one block's 5x5
        // pixels we see both lit and dark pixels (a smooth, interpolated shadow).
        let mut found_partial = false;
        for z in 0..grid_h {
            for x in 0..grid_w {
                let mut lit_cnt = 0;
                let mut dark_cnt = 0;
                for dz in 0..s {
                    for dx in 0..s {
                        let c = img.get_pixel((x * s + dx) as u32, (z * s + dz) as u32).0;
                        if c == dark {
                            dark_cnt += 1;
                        } else if c == lit {
                            lit_cnt += 1;
                        }
                    }
                }
                if lit_cnt > 0 && dark_cnt > 0 {
                    found_partial = true;
                }
            }
        }
        assert!(
            found_partial,
            "a block should be partially shadowed along the smooth edge"
        );
    }

    #[test]
    fn hypersample_renders_solid_15x15_blocks_without_interpolation() {
        // Only block (x=0, z=0) is grass; everything else is void.
        let top = ChunkTop {
            blocks: std::array::from_fn(|i| {
                (i == 0).then(|| "minecraft:grass_block".to_string())
            }),
            heights: [None; COLUMNS],
            under: std::array::from_fn(|_| None),
        };

        let path = std::env::temp_dir().join("worldraw_test_hs.png");
        render_chunk_png_ss(&top, &path, HYPER_SAMPLE, false, false).expect("render should succeed");

        let img = image::open(&path).expect("png should be readable");
        assert_eq!(img.dimensions(), (16 * 15, 16 * 15));

        // The grass block at (0,0) is a solid 15x15 grass square.
        for z in 0..15u32 {
            for x in 0..15u32 {
                assert_eq!(img.get_pixel(x, z).0, [106, 170, 64, 255]);
            }
        }
        // The neighbouring block (1,0) is void -> black.
        assert_eq!(img.get_pixel(15, 0).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(239, 239).0, [0, 0, 0, 255]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn render_ss_applies_ambient_occlusion_gradient() {
        let grid_w = 2;
        let grid_h = 1;
        let s = 16usize;
        let heights = vec![0i32, 1];
        let colors = vec![
            block_color("minecraft:sand"),
            block_color("minecraft:sand"),
        ];
        let ao = ambient_occlusion(&heights, grid_w, grid_h, s);

        let mut img = RgbImage::new((grid_w * s) as u32, (grid_h * s) as u32);
        render_ss(&mut img, &colors, None, Some(&ao), None, grid_w, grid_h, s, false);

        let lit = block_color("minecraft:sand");
        // The lower block's edge pixel (facing the higher block) is darkened.
        let edge = img.get_pixel((s - 1) as u32, 0).0;
        assert!(
            edge[0] < lit[0] && edge[1] < lit[1] && edge[2] < lit[2],
            "edge should be darker than the lit color"
        );
        // The higher block is unchanged.
        assert_eq!(img.get_pixel((s + s / 2) as u32, 0).0, lit);
    }

    #[test]
    fn chunk_png_supersampled_with_ao_darkens_the_lower_edge() {
        // A 16x16 chunk: block (x=0, z=0) at height 0, block (x=1, z=0) at
        // height 1, everything else void. The lower block's right edge should
        // be darkened, the higher block should stay lit.
        let mut blocks: [Option<String>; COLUMNS] =
            std::array::from_fn(|_| None);
        let mut heights: [Option<i32>; COLUMNS] = [None; COLUMNS];
        blocks[0] = Some("minecraft:sand".to_string());
        heights[0] = Some(0);
        blocks[1] = Some("minecraft:sand".to_string());
        heights[1] = Some(1);

        let top = ChunkTop {
            blocks,
            heights,
            under: std::array::from_fn(|_| None),
        };
        let path = std::env::temp_dir().join("worldraw_test_ao.png");
        render_chunk_png_ss_fx(&top, &path, 16, false, true, false, false)
            .expect("render should succeed");

        let img = image::open(&path).expect("png should be readable");
        let s = 16u32;
        let lit = [219u8, 209, 160, 255]; // sand (RGBA)

        // Higher block (x=1) centre stays fully lit.
        assert_eq!(img.get_pixel(s + s / 2, s / 2).0, lit);
        // Lower block (x=0) right edge is darkened.
        let edge = img.get_pixel(s - 1, s / 2).0;
        assert!(
            edge[0] < lit[0] && edge[1] < lit[1] && edge[2] < lit[2],
            "lower block's facing edge should be darkened, got {edge:?}"
        );
        // Lower block far from the edge stays lit.
        assert_eq!(img.get_pixel(0, s / 2).0, lit);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn render_ss_applies_bloom_glow_around_a_light() {
        let grid_w = 5;
        let grid_h = 1;
        let s = 4usize;
        let pw = grid_w * s;
        // A black field with a single torch at block (x=2, z=0).
        let colors = vec![[0u8, 0, 0]; grid_w * grid_h];
        let mut lights = vec![[0u8, 0, 0]; grid_w * grid_h];
        lights[2] = [255, 200, 90];
        let bloom_field = bloom(&lights, grid_w, grid_h, s, BLOOM_RADIUS_BLOCKS);

        let mut img = RgbImage::new(pw as u32, (grid_h * s) as u32);
        render_ss(
            &mut img,
            &colors,
            None,
            None,
            Some(&bloom_field),
            grid_w,
            grid_h,
            s,
            false,
        );

        // The torch's own square glows (base black + full bloom at its centre).
        let center = img.get_pixel(2 * s as u32 + s as u32 / 2, 0).0;
        assert!(
            center[0] > 200 && center[1] > 150,
            "the light block should glow, got {center:?}"
        );
        // A dark pixel inside the radius is brightened by the warm bloom.
        let halo = img.get_pixel((2 * s + s / 2 + 2) as u32, 0).0;
        assert_ne!(
            halo,
            [0, 0, 0],
            "a pixel inside the radius should be lit by the bloom"
        );
        // A pixel beyond the radius stays black.
        assert_eq!(
            img.get_pixel(0, 0).0,
            [0, 0, 0],
            "outside the radius stays unlit"
        );
    }
}