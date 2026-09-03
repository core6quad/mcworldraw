//! Region-file handling: per-file chunk scanning, single-mode compositing, and
//! the per-chunk PNG rendering driver.
//!
//! Each `r.X.Z.mca` region file holds a 32x32 grid of chunks. These helpers
//! read a single region file and either render each of its chunks to its own
//! PNG (`process_region`), or contribute to the shared world image in
//! `--single` mode: `collect_grid` fills the global height/color grids used by
//! the shadow and ambient-occlusion passes, while `process_region_single` and
//! `process_region_single_ss` blit chunks into the in-memory output image.

use std::fs;
use std::path::Path;

use fastnbt::from_bytes;
use image::RgbImage;
use indicatif::ProgressBar;
use mca::RegionReader;

use crate::chunk::{get_top_blocks, Chunk, CHUNK_SIZE, VOID_H};
use crate::color::{display_color, light_bloom_color, night_darken, NO_BLOCK};
use crate::render::{
    blit_supersampled, render_chunk_png, render_chunk_png_ss, render_chunk_png_ss_fx, render_into_big,
};

/// Bounding box of chunk coordinates, in chunk units (not blocks).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    pub(crate) min_x: i32,
    pub(crate) min_z: i32,
    pub(crate) max_x: i32,
    pub(crate) max_z: i32,
}

impl Bounds {
    /// The bounding box that encloses both `self` and `other`.
    pub(crate) fn merged(&self, other: &Self) -> Self {
        Bounds {
            min_x: self.min_x.min(other.min_x),
            min_z: self.min_z.min(other.min_z),
            max_x: self.max_x.max(other.max_x),
            max_z: self.max_z.max(other.max_z),
        }
    }
}

/// Decode a region file and write one PNG per chunk it contains, returning the
/// number of chunk PNGs generated.
///
/// `upsample > 1` switches to the supersampled renderer (each block becomes a
/// solid `upsample x upsample` square); `ambient_occlusion` layers the soft
/// edge darkening on top of that and `bloom` layers a radial light gradient
/// around light-emitting blocks. Otherwise each chunk is drawn at `scale`.
pub(crate) fn process_region(
    path: &Path,
    out_dir: &Path,
    scale: u32,
    upsample: u32,
    dim_prefix: &str,
    ambient_occlusion: bool,
    bloom: bool,
    transparency: bool,
    night: bool,
    pb: &ProgressBar,
) -> Result<usize, Box<dyn std::error::Error>> {
    let Some((region_x, region_z)) = parse_region_coords(path) else {
        eprintln!("Skipping invalid region filename: {}", path.display());
        return Ok(0);
    };

    let data = fs::read(path)?;

    // Empty (0-byte) region files show up frequently as placeholders, and
    // some may be malformed. Skip them instead of aborting the whole run.
    let mut region = match RegionReader::new(&data) {
        Ok(region) => region,
        Err(e) => {
            eprintln!("Skipping region {}: {e}", path.display());
            return Ok(0);
        }
    };

    pb.set_message(format!("r.{region_x}.{region_z}"));

    let mut generated = 0usize;

    for local_z in 0..32u8 {
        for local_x in 0..32u8 {
            let Some(chunk_data) = region.chunk(local_x, local_z)? else {
                continue;
            };

            let chunk_x = region_x * 32 + local_x as i32;
            let chunk_z = region_z * 32 + local_z as i32;

            let chunk: Chunk = from_bytes(chunk_data)?;

            let top = get_top_blocks(&chunk);

            let out_path = out_dir.join(format!("{dim_prefix}c_{chunk_x}_{chunk_z}.png"));
            if upsample > 1 {
                if ambient_occlusion || bloom {
                    render_chunk_png_ss_fx(
                        &top,
                        &out_path,
                        upsample,
                        transparency,
                        ambient_occlusion,
                        bloom,
                        night,
                    )?;
                } else {
                    render_chunk_png_ss(&top, &out_path, upsample, transparency, night)?;
                }
            } else {
                render_chunk_png(&top, &out_path, scale, transparency, night)?;
            }

            generated += 1;
            pb.inc(1);
        }
    }

    Ok(generated)
}

/// Best-effort scan of a region file without rendering.
///
/// Returns `(count, bounds)`: `count` is the number of non-empty chunks in the
/// file and `bounds` the bounding box (in chunk coordinates) of those chunks,
/// or `None` when the file is unreadable, malformed, or holds no chunks.
pub(crate) fn scan_region(path: &Path) -> (usize, Option<Bounds>) {
    let Some((region_x, region_z)) = parse_region_coords(path) else {
        return (0, None);
    };

    let Ok(data) = fs::read(path) else {
        return (0, None);
    };

    let Ok(mut region) = RegionReader::new(&data) else {
        return (0, None);
    };

    let mut count = 0usize;
    let mut min_x = i32::MAX;
    let mut min_z = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_z = i32::MIN;

    for local_z in 0..32u8 {
        for local_x in 0..32u8 {
            if let Ok(Some(_)) = region.chunk(local_x, local_z) {
                count += 1;

                let chunk_x = region_x * 32 + local_x as i32;
                let chunk_z = region_z * 32 + local_z as i32;

                min_x = min_x.min(chunk_x);
                max_x = max_x.max(chunk_x);
                min_z = min_z.min(chunk_z);
                max_z = max_z.max(chunk_z);
            }
        }
    }

    let bounds = if count > 0 {
        Some(Bounds {
            min_x,
            min_z,
            max_x,
            max_z,
        })
    } else {
        None
    };

    (count, bounds)
}

/// Parse a `r.X.Z.mca` region file name into `(region_x, region_z)`.
///
/// Region files are named `r.<x>.<z>.mca` where `x`/`z` may be negative. Any
/// other file name (e.g. a stray temp file) is rejected.
pub(crate) fn parse_region_coords(path: &Path) -> Option<(i32, i32)> {
    let filename = path.file_stem()?.to_str()?;

    let parts: Vec<&str> = filename.split('.').collect();

    if parts.len() != 3 || parts[0] != "r" {
        return None;
    }

    let x: i32 = parts[1].parse().ok()?;
    let z: i32 = parts[2].parse().ok()?;

    Some((x, z))
}

/// Fill the shared `heights` / `colors` / `lights` grids (in block units,
/// covering the world bounding box) with this region file's chunk data.
///
/// `grid_w` is the width of the grids in blocks; `min_chunk_x` / `min_chunk_z`
/// are the world's minimum chunk coordinates. A `None` column is recorded as
/// [`VOID_H`] with the no-block color. `lights` holds each column's bloom color
/// (or the no-block color when it does not emit light).
pub(crate) fn collect_grid(
    path: &Path,
    heights: &mut [i32],
    colors: &mut [[u8; 3]],
    lights: &mut [[u8; 3]],
    grid_w: usize,
    min_chunk_x: i32,
    min_chunk_z: i32,
    transparency: bool,
    night: bool,
    pb: &ProgressBar,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some((region_x, region_z)) = parse_region_coords(path) else {
        eprintln!("Skipping invalid region filename: {}", path.display());
        return Ok(());
    };

    let data = fs::read(path)?;

    let mut region = match RegionReader::new(&data) {
        Ok(region) => region,
        Err(e) => {
            eprintln!("Skipping region {}: {e}", path.display());
            return Ok(());
        }
    };

    pb.set_message(format!("r.{region_x}.{region_z}"));

    for local_z in 0..32u8 {
        for local_x in 0..32u8 {
            let Some(chunk_data) = region.chunk(local_x, local_z)? else {
                continue;
            };

            let chunk_x = region_x * 32 + local_x as i32;
            let chunk_z = region_z * 32 + local_z as i32;

            let chunk: Chunk = from_bytes(chunk_data)?;
            let top = get_top_blocks(&chunk);

            let block_x0 = (chunk_x - min_chunk_x) * (CHUNK_SIZE as i32);
            let block_z0 = (chunk_z - min_chunk_z) * (CHUNK_SIZE as i32);

            for lz in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let gx = (block_x0 + lx as i32) as usize;
                    let gz = (block_z0 + lz as i32) as usize;
                    let gi = gz * grid_w + gx;

                    let ci = lz * CHUNK_SIZE + lx;
                    heights[gi] = top.heights[ci].unwrap_or(VOID_H);
                    let c = display_color(&top.blocks[ci], &top.under[ci], transparency);
                    colors[gi] = if night { night_darken(c) } else { c };
                    lights[gi] = top
                        .blocks[ci]
                        .as_deref()
                        .and_then(light_bloom_color)
                        .unwrap_or(NO_BLOCK);
                }
            }

            pb.inc(1);
        }
    }

    Ok(())
}

/// Render every chunk of a region into the shared single-map image (native
/// resolution, no shadows / ambient occlusion), downsampled by `scale`.
pub(crate) fn process_region_single(
    path: &Path,
    img: &mut RgbImage,
    min_chunk_x: i32,
    min_chunk_z: i32,
    scale: u32,
    transparency: bool,
    night: bool,
    pb: &ProgressBar,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some((region_x, region_z)) = parse_region_coords(path) else {
        eprintln!("Skipping invalid region filename: {}", path.display());
        return Ok(());
    };

    let data = fs::read(path)?;

    // Empty (0-byte) region files show up frequently as placeholders, and
    // some may be malformed. Skip them instead of aborting the whole run.
    let mut region = match RegionReader::new(&data) {
        Ok(region) => region,
        Err(e) => {
            eprintln!("Skipping region {}: {e}", path.display());
            return Ok(());
        }
    };

    pb.set_message(format!("r.{region_x}.{region_z}"));

    for local_z in 0..32u8 {
        for local_x in 0..32u8 {
            let Some(chunk_data) = region.chunk(local_x, local_z)? else {
                continue;
            };

            let chunk_x = region_x * 32 + local_x as i32;
            let chunk_z = region_z * 32 + local_z as i32;

            let chunk: Chunk = from_bytes(chunk_data)?;

            let top = get_top_blocks(&chunk);

            let s = scale as i32;
            let offset_x = ((chunk_x - min_chunk_x) * (CHUNK_SIZE as i32)) / s;
            let offset_z = ((chunk_z - min_chunk_z) * (CHUNK_SIZE as i32)) / s;

            render_into_big(
                img,
                &top,
                offset_x as u32,
                offset_z as u32,
                scale,
                transparency,
                night,
            );

            pb.inc(1);
        }
    }

    Ok(())
}

/// Decode a region and composite each of its chunks into a shared big image in
/// supersample mode: every block becomes a solid `ss x ss` pixel square, so a
/// chunk sits at pixel offset `(chunk - min) * CHUNK_SIZE * ss` in the image.
pub(crate) fn process_region_single_ss(
    path: &Path,
    img: &mut RgbImage,
    min_chunk_x: i32,
    min_chunk_z: i32,
    ss: i32,
    transparency: bool,
    night: bool,
    pb: &ProgressBar,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some((region_x, region_z)) = parse_region_coords(path) else {
        eprintln!("Skipping invalid region filename: {}", path.display());
        return Ok(());
    };

    let data = fs::read(path)?;

    // Empty (0-byte) region files show up frequently as placeholders, and
    // some may be malformed. Skip them instead of aborting the whole run.
    let mut region = match RegionReader::new(&data) {
        Ok(region) => region,
        Err(e) => {
            eprintln!("Skipping region {}: {e}", path.display());
            return Ok(());
        }
    };

    pb.set_message(format!("r.{region_x}.{region_z}"));

    for local_z in 0..32u8 {
        for local_x in 0..32u8 {
            let Some(chunk_data) = region.chunk(local_x, local_z)? else {
                continue;
            };

            let chunk_x = region_x * 32 + local_x as i32;
            let chunk_z = region_z * 32 + local_z as i32;

            let chunk: Chunk = from_bytes(chunk_data)?;
            let top = get_top_blocks(&chunk);

            let offset_x =
                ((chunk_x - min_chunk_x) * (CHUNK_SIZE as i32) * ss) as u32;
            let offset_z =
                ((chunk_z - min_chunk_z) * (CHUNK_SIZE as i32) * ss) as u32;

            blit_supersampled(
                img,
                &top.blocks,
                &top.under,
                CHUNK_SIZE,
                CHUNK_SIZE,
                ss as usize,
                offset_x,
                offset_z,
                transparency,
                night,
            );

            pb.inc(1);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_region_coords_reads_coordinates_from_filename() {
        assert_eq!(
            parse_region_coords(Path::new("region/r.1.-2.mca")),
            Some((1, -2))
        );
        assert_eq!(
            parse_region_coords(Path::new("region/r.0.0.mca")),
            Some((0, 0))
        );
        assert_eq!(
            parse_region_coords(Path::new("region/r.-5.-99.mca")),
            Some((-5, -99))
        );
        // Not a region file at all.
        assert_eq!(parse_region_coords(Path::new("region/foo.mca")), None);
        // Wrong number of components.
        assert_eq!(parse_region_coords(Path::new("region/r.1.mca")), None);
        // Non-integer coordinates.
        assert_eq!(
            parse_region_coords(Path::new("region/r.x.z.mca")),
            None
        );
    }

    #[test]
    fn bounds_merge_covers_both_boxes() {
        let a = Bounds {
            min_x: 0,
            min_z: 0,
            max_x: 2,
            max_z: 2,
        };
        let b = Bounds {
            min_x: -3,
            min_z: 1,
            max_x: 1,
            max_z: 5,
        };
        let merged = a.merged(&b);
        assert_eq!(merged.min_x, -3);
        assert_eq!(merged.min_z, 0);
        assert_eq!(merged.max_x, 2);
        assert_eq!(merged.max_z, 5);
    }
}