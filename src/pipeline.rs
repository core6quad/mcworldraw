//! Shared render pipeline used by both the CLI entry point and the dedicated
//! server.
//!
//! [`run_render`] holds the whole "locate the region files, scan them, and
//! composite the map" logic that used to live inline in `main`. It is driven by
//! a [`RenderConfig`] and reports progress through an
//! [`indicatif::ProgressBar`], so the CLI can show a console bar while the
//! server mirrors the same numbers to the browser over HTTP.

use std::fs;
use std::path::{Path, PathBuf};

use image::RgbImage;
use indicatif::ProgressBar;

use crate::args::{Args, HYPER_SAMPLE, SUPER_SAMPLE};
use crate::chunk::{CHUNK_SIZE, VOID_H};
use crate::color::NO_BLOCK;
use crate::dimension::{dimension_info, dimension_region_path};
use crate::light::{
    ambient_occlusion, bloom, compute_shadows, supersampled_heights, BLOOM_RADIUS_BLOCKS,
    NIGHT_BLOOM_RADIUS_BLOCKS,
};
use crate::region::{
    collect_grid, count_region_index, process_region, process_region_single,
    process_region_single_ss, scan_region, Bounds,
};
use crate::render::{render_shaded_map, render_ss};

/// Everything needed to render one map. Built from CLI arguments or from the
/// web form, then consumed by [`run_render`].
pub(crate) struct RenderConfig {
    /// World root directory (the folder containing `region/`, `dimensions/`, …).
    pub world_path: PathBuf,
    /// Dimension index: `0` overworld, `1` nether, `-1` end.
    pub dim: i32,
    /// `true` for a single combined PNG, `false` for per-chunk PNGs.
    pub single: bool,
    /// Blocks per pixel; only meaningful when no (hyper)supersampling is on.
    pub scale: u32,
    pub shadows: bool,
    pub supersample: bool,
    pub hypersample: bool,
    pub ambient_occlusion: bool,
    pub bloom: bool,
    pub transparency: bool,
    pub night: bool,
    /// Directory the resulting PNG(s) are written to.
    pub output_dir: PathBuf,
}

impl RenderConfig {
    /// Build a config from parsed CLI arguments (the CLI writes to `output/`).
    pub(crate) fn from_args(args: &Args) -> Self {
        RenderConfig {
            world_path: PathBuf::from(args.world_path.clone()),
            dim: args.dim,
            single: args.single,
            scale: args.scale,
            shadows: args.shadows,
            supersample: args.supersample,
            hypersample: args.hypersample,
            ambient_occlusion: args.ambient_occlusion,
            bloom: args.bloom,
            transparency: args.transparency,
            night: args.night,
            output_dir: PathBuf::from("output"),
        }
    }

    /// Pixels-per-block upsample factor: `Some(15)` / `Some(5)` / `None`.
    pub(crate) fn upsample_factor(&self) -> Option<u32> {
        if self.hypersample {
            Some(HYPER_SAMPLE)
        } else if self.supersample {
            Some(SUPER_SAMPLE)
        } else {
            None
        }
    }
}

/// Result of a completed [`run_render`].
pub(crate) struct RenderResult {
    pub dim_name: &'static str,
    pub region_files: usize,
    pub chunks: usize,
    /// Output image size (single mode only).
    pub width: u32,
    pub height: u32,
    /// The single PNG path (single mode) when one was produced.
    pub single_image: Option<PathBuf>,
    /// The output directory holding per-chunk PNGs (chunk mode) when produced.
    pub chunk_dir: Option<PathBuf>,
    /// The single-image file name (e.g. `world.png`) for the chosen dimension.
    pub out_file_name: &'static str,
}

impl RenderResult {
    fn empty(dim_name: &'static str, region_files: usize, out_file_name: &'static str) -> Self {
        RenderResult {
            dim_name,
            region_files,
            chunks: 0,
            width: 0,
            height: 0,
            single_image: None,
            chunk_dir: None,
            out_file_name,
        }
    }
}

/// Per-dimension statistics about a world's region data, used to preview how
/// large a render will be *before* it runs (see [`scan_dimension_stats`]).
pub(crate) struct DimStats {
    /// Number of non-empty chunks in this dimension.
    pub chunks: usize,
    /// Width of the chunk bounding box, in blocks.
    pub blocks_w: u64,
    /// Depth (z) of the chunk bounding box, in blocks.
    pub blocks_h: u64,
    /// Number of `.mca` region files on disk for this dimension.
    pub region_files: usize,
}

impl DimStats {
    fn empty() -> Self {
        DimStats {
            chunks: 0,
            blocks_w: 0,
            blocks_h: 0,
            region_files: 0,
        }
    }
}

/// Scan one dimension's region files (index only — no chunk decompression) and
/// report how many non-empty chunks it holds and the size of their bounding box
/// in blocks. Mirrors the first pass of [`run_render`] but is cheap enough to
/// run for every dimension right after an upload.
pub(crate) fn scan_dimension_stats(world_path: &Path, dim: i32) -> DimStats {
    let Ok(info) = dimension_info(dim) else {
        return DimStats::empty();
    };
    let Some(region_path) = dimension_region_path(world_path, &info) else {
        return DimStats::empty();
    };

    let mut region_files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&region_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "mca") {
                region_files.push(path);
            }
        }
    }
    region_files.sort();

    let mut total_chunks = 0usize;
    let mut world_bounds: Option<Bounds> = None;
    for path in &region_files {
        let (count, bounds) = count_region_index(path);
        total_chunks += count;
        if let Some(b) = bounds {
            world_bounds = Some(match world_bounds {
                Some(prev) => prev.merged(&b),
                None => b,
            });
        }
    }

    let (blocks_w, blocks_h) = match world_bounds {
        Some(b) => {
            let w = ((b.max_x - b.min_x + 1) as i64 * CHUNK_SIZE as i64).max(0) as u64;
            let h = ((b.max_z - b.min_z + 1) as i64 * CHUNK_SIZE as i64).max(0) as u64;
            (w, h)
        }
        None => (0, 0),
    };

    DimStats {
        chunks: total_chunks,
        blocks_w,
        blocks_h,
        region_files: region_files.len(),
    }
}

/// Validate the combination of renderer flags, mirroring the CLI rules:
/// (super/hyper)sample cannot be combined with each other, and ambient
/// occlusion / bloom require a (hyper)supersampled pass.
pub(crate) fn validate_config(config: &RenderConfig) -> Result<(), String> {
    if config.supersample && config.hypersample {
        return Err("Supersample and hypersampling cannot be combined".into());
    }
    if config.scale < 1 {
        return Err("Scale must be >= 1".into());
    }
    if config.ambient_occlusion && !(config.supersample || config.hypersample) {
        return Err("Ambient occlusion requires supersampling or hypersampling".into());
    }
    if config.bloom && !(config.supersample || config.hypersample) {
        return Err("Bloom requires supersampling or hypersampling".into());
    }
    Ok(())
}

/// Run the full render pipeline: locate the dimension's region files, scan
/// them for chunks and bounds, then produce either a single PNG or a set of
/// per-chunk PNGs in `config.output_dir`. Progress is reported through `pb`.
///
/// A `chunks == 0` result means the world had no matching region data (not an
/// error); anything that genuinely cannot render is returned as `Err`.
pub(crate) fn run_render(config: &RenderConfig, pb: &ProgressBar) -> Result<RenderResult, String> {
    let dim = dimension_info(config.dim).map_err(|e| e.to_string())?;

    let region_path = dimension_region_path(&config.world_path, &dim).ok_or_else(|| {
        let tried = dim
            .region_candidates
            .iter()
            .map(|cand| config.world_path.join(*cand).display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Region directory does not exist for {}. Looked for: {tried}",
            dim.name
        )
    })?;

    let mut region_files: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&region_path).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|ext| ext == "mca") {
            region_files.push(path);
        }
    }
    region_files.sort();

    fs::create_dir_all(&config.output_dir).map_err(|e| e.to_string())?;

    // First pass: count chunks (for a determinate progress bar) and track the
    // world's bounding box (to size a single image).
    let mut total_chunks = 0usize;
    let mut world_bounds: Option<Bounds> = None;
    for path in &region_files {
        let (count, bounds) = scan_region(path);
        total_chunks += count;
        if let Some(b) = bounds {
            world_bounds = Some(match world_bounds {
                Some(prev) => prev.merged(&b),
                None => b,
            });
        }
    }

    pb.set_length(total_chunks as u64);

    if total_chunks == 0 {
        return Ok(RenderResult::empty(dim.name, region_files.len(), dim.out_file));
    }

    if config.single {
        let bounds = world_bounds.unwrap();

        // (Hyper)supersampling fixes a pixel-per-block resolution; otherwise the
        // user's scale factor decides how many blocks collapse into a pixel.
        let scale = config.scale as i32;
        let upsample = config.upsample_factor();
        let ss = upsample.unwrap_or(1) as i32;
        let blocks_w = (bounds.max_x - bounds.min_x + 1) * (CHUNK_SIZE as i32);
        let blocks_h = (bounds.max_z - bounds.min_z + 1) * (CHUNK_SIZE as i32);
        let width = if upsample.is_some() {
            blocks_w * ss
        } else {
            (blocks_w + scale - 1) / scale
        };
        let height = if upsample.is_some() {
            blocks_h * ss
        } else {
            (blocks_h + scale - 1) / scale
        };
        let (width, height) = (width as u32, height as u32);

        let mut img = RgbImage::new(width, height);

        if config.shadows || config.ambient_occlusion || config.bloom {
            // Pixel grid (blocks), then map to the actual pixel canvas after the
            // optional upsampled shadow / AO / bloom passes.
            let grid_w = blocks_w as usize;
            let grid_h = blocks_h as usize;
            let n = grid_w * grid_h;
            let mut heights = vec![VOID_H; n];
            let mut colors = vec![NO_BLOCK; n];
            let mut lights = vec![NO_BLOCK; n];

            for path in &region_files {
                collect_grid(
                    path,
                    &mut heights,
                    &mut colors,
                    &mut lights,
                    grid_w,
                    bounds.min_x,
                    bounds.min_z,
                    config.transparency,
                    config.night,
                    pb,
                )
                .map_err(|e| e.to_string())?;
            }

            if let Some(factor) = upsample {
                let s = factor as usize;
                let pw = grid_w * s;
                let ph = grid_h * s;
                let pixel_heights = supersampled_heights(&heights, grid_w, grid_h, s);
                let pixel_shadow = if config.shadows {
                    Some(compute_shadows(&pixel_heights, pw, ph))
                } else {
                    None
                };
                drop(pixel_heights);
                let pixel_ao = if config.ambient_occlusion {
                    Some(ambient_occlusion(&heights, grid_w, grid_h, s))
                } else {
                    None
                };
                drop(heights);
                let pixel_bloom = if config.bloom {
                    let radius = if config.night {
                        NIGHT_BLOOM_RADIUS_BLOCKS
                    } else {
                        BLOOM_RADIUS_BLOCKS
                    };
                    Some(bloom(&lights, grid_w, grid_h, s, radius))
                } else {
                    None
                };
                drop(lights);

                render_ss(
                    &mut img,
                    &colors,
                    pixel_shadow.as_deref(),
                    pixel_ao.as_deref(),
                    pixel_bloom.as_deref(),
                    grid_w,
                    grid_h,
                    s,
                    config.night,
                );
            } else {
                let shadow = compute_shadows(&heights, grid_w, grid_h);
                drop(heights);

                render_shaded_map(
                    &mut img,
                    &colors,
                    &shadow,
                    grid_w,
                    bounds.min_x,
                    bounds.min_z,
                    bounds.max_x,
                    bounds.max_z,
                    config.scale,
                    config.night,
                );
            }
        } else if let Some(factor) = upsample {
            let s = factor as i32;
            for path in &region_files {
                process_region_single_ss(
                    path,
                    &mut img,
                    bounds.min_x,
                    bounds.min_z,
                    s,
                    config.transparency,
                    config.night,
                    pb,
                )
                .map_err(|e| e.to_string())?;
            }
        } else {
            for path in &region_files {
                process_region_single(
                    path,
                    &mut img,
                    bounds.min_x,
                    bounds.min_z,
                    config.scale,
                    config.transparency,
                    config.night,
                    pb,
                )
                .map_err(|e| e.to_string())?;
            }
        }

        let out_path = config.output_dir.join(dim.out_file);
        img.save(&out_path).map_err(|e| e.to_string())?;

        return Ok(RenderResult {
            dim_name: dim.name,
            region_files: region_files.len(),
            chunks: total_chunks,
            width,
            height,
            single_image: Some(out_path),
            chunk_dir: None,
            out_file_name: dim.out_file,
        });
    }

    // Per-chunk mode.
    for path in &region_files {
        let _ = process_region(
            path,
            &config.output_dir,
            config.scale,
            config.upsample_factor().unwrap_or(1),
            dim.out_prefix,
            config.ambient_occlusion,
            config.bloom,
            config.transparency,
            config.night,
            pb,
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(RenderResult {
        dim_name: dim.name,
        region_files: region_files.len(),
        chunks: total_chunks,
        width: 0,
        height: 0,
        single_image: None,
        chunk_dir: Some(config.output_dir.clone()),
        out_file_name: dim.out_file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_index_mca(path: &Path, slots: &[(usize, u32)]) {
        let mut index = [0u8; 4096];
        for (entry, offset) in slots {
            let b = entry * 4;
            index[b] = (offset >> 16) as u8;
            index[b + 1] = (offset >> 8) as u8;
            index[b + 2] = (*offset & 0xff) as u8;
            index[b + 3] = 1;
        }
        fs::write(path, index).unwrap();
    }

    #[test]
    fn scan_dimension_stats_reports_chunks_and_block_span() {
        let dir = std::env::temp_dir().join(format!("worldraw-ci-scan-{}", std::process::id()));
        let region_dir = dir.join("region");
        let _ = fs::create_dir_all(&region_dir);
        // Region (1,2): occupied chunks at (32,64) and (63,65).
        write_index_mca(&region_dir.join("r.1.2.mca"), &[(0, 2), (63, 3)]);

        let s = scan_dimension_stats(&dir, 0);
        assert_eq!(s.chunks, 2);
        assert_eq!(s.region_files, 1);
        // Chunks span x in [32..63] (32 chunks) and z in [64..65] (2 chunks).
        assert_eq!(s.blocks_w, 32 * CHUNK_SIZE as u64);
        assert_eq!(s.blocks_h, 2 * CHUNK_SIZE as u64);

        // A dimension with no matching region folder is empty.
        let empty = scan_dimension_stats(&dir, -1);
        assert_eq!(empty.chunks, 0);
        assert_eq!(empty.region_files, 0);

        let _ = fs::remove_dir_all(&dir);
    }
}
