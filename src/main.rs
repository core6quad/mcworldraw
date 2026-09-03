use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use fastnbt::{from_bytes, LongArray};
use image::{Rgb, RgbImage};
use indicatif::{ProgressBar, ProgressStyle};
use mca::RegionReader;
use serde::Deserialize;

const CHUNK_SIZE: usize = 16;
const COLUMNS: usize = 256;

/// Color used for columns that have no non-air block (i.e. pure void).
const NO_BLOCK: [u8; 3] = [0, 0, 0];

/// Sentinel surface height for a column that has no block (pure void).
///
/// Real column heights are actual Minecraft Y values, so a value far below any
/// possible Y marks "no block" without colliding with real terrain. Used by the
/// shadow pass so void columns neither cast nor receive a shadow.
const VOID_H: i32 = -1_000_000;

/// How strongly a shadowed surface is darkened: each RGB channel is multiplied
/// by `SHADOW_NUMERATOR / SHADOW_DENOMINATOR`.
const SHADOW_NUMERATOR: u32 = 55;
const SHADOW_DENOMINATOR: u32 = 100;

/// Supersampling factor for `--supersample`: each block is drawn as a solid
/// `SUPER_SAMPLE x SUPER_SAMPLE` pixel square (5 pixels per block) with no
/// interpolation between blocks.
const SUPER_SAMPLE: u32 = 5;

#[derive(Debug, Deserialize)]
struct Chunk {
    #[serde(default)]
    sections: Vec<Section>,
}

#[derive(Debug, Deserialize)]
struct Section {
    #[serde(rename = "Y")]
    y: i8,

    #[serde(default)]
    block_states: Option<BlockStates>,
}

#[derive(Debug, Deserialize)]
struct BlockStates {
    palette: Vec<PaletteEntry>,

    #[serde(default)]
    data: Option<LongArray>,
}

#[derive(Debug, Deserialize)]
struct PaletteEntry {
    #[serde(rename = "Name")]
    name: String,
}

/// The top block for every x/z column in a chunk.
///
/// Index = z * 16 + x
#[derive(Clone, Debug)]
struct ChunkTop {
    /// Minecraft block name, e.g. "minecraft:grass_block"
    blocks: [Option<String>; COLUMNS],

    /// Absolute Minecraft Y coordinate of the top block (the column's surface
    /// height), or `None` for a void column.
    ///
    /// Used by the shadow pass to compute elevation-based shadows.
    heights: [Option<i32>; COLUMNS],
}

/// Bounding box of chunk coordinates, in chunk units (not blocks).
#[derive(Clone, Copy, Debug)]
struct Bounds {
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
}

impl Bounds {
    /// The bounding box that encloses both `self` and `other`.
    fn merged(&self, other: &Self) -> Self {
        Bounds {
            min_x: self.min_x.min(other.min_x),
            min_z: self.min_z.min(other.min_z),
            max_x: self.max_x.max(other.max_x),
            max_z: self.max_z.max(other.max_z),
        }
    }
}

/// Parsed command-line arguments.
struct Args {
    world_path: String,
    single: bool,
    scale: u32,
    dim: i32,
    shadows: bool,
    supersample: bool,
}

fn print_usage() {
    println!(
        "Usage: mcmap <path-to-world> [options]\n\n\
         Renders a top-down map of a Minecraft world from its region files.\n\n\
         Options:\n\
         -s, --single         Render one big PNG instead of one PNG per chunk\n\
         -z, --scale <N>      Downsample so each pixel is N x N blocks; the\n\
                              pixel color is the most common block in the area\n\
                              (1 = one pixel per block, the default)\n\
         -d, --dim <N>        Dimension id to render: 0 = overworld (default),\n\
                              1 = the nether, -1 = the end\n\
         -r, --shadows        Render diagonal elevation shadows as if the sun\n\
                              were 45 degrees up at the top-right (only applies\n\
                              in single mode, i.e. together with -s)\n\
         -ss, --supersample   Render each block as a solid 5x5 pixel square\n\
                              (5 pixels per block, no interpolation). When\n\
                              shadows are enabled they are resolved at pixel\n\
                              resolution so their edges stay smooth. Cannot be\n\
                              combined with -z/--scale\n\
         -h, --help           Show this message"
    );
}

fn parse_args() -> Result<Args, String> {
    let mut world_path: Option<String> = None;
    let mut single = false;
    let mut scale: u32 = 1;
    let mut scale_set = false;
    let mut dim: i32 = 0;
    let mut shadows = false;
    let mut supersample = false;

    let raw: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;

    while i < raw.len() {
        let arg = raw[i].as_str();

        match arg {
            "--single" | "--big" | "-s" => single = true,
            "--shadows" | "--shadow" | "-r" => shadows = true,
            "--supersample" | "-ss" => supersample = true,
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--scale" | "-z" => {
                i += 1;
                let value = raw
                    .get(i)
                    .ok_or("--scale requires a value (e.g. --scale 2)")?;
                scale = parse_scale(value)?;
                scale_set = true;
            }
            _ if arg.starts_with("--scale=") => {
                scale = parse_scale(&arg["--scale=".len()..])?;
                scale_set = true;
            }
            "--dim" | "-d" => {
                i += 1;
                let value = raw
                    .get(i)
                    .ok_or("--dim requires a value (e.g. --dim 1)")?;
                dim = parse_dim(value)?;
            }
            _ if arg.starts_with("--dim=") => {
                dim = parse_dim(&arg["--dim=".len()..])?;
            }
            other => {
                if world_path.is_some() {
                    return Err(format!("Unexpected argument: {other}"));
                }
                world_path = Some(other.to_string());
            }
        }

        i += 1;
    }

    if supersample && scale_set {
        return Err(
            "--supersample (-ss) cannot be combined with --scale (-z/--scale)".into(),
        );
    }

    let world_path =
        world_path.ok_or_else(|| "Missing <path-to-world> argument".to_string())?;

    Ok(Args { world_path, single, scale, dim, shadows, supersample })
}

/// Parse and validate a `--scale` value: a positive integer (>= 1).
fn parse_scale(value: &str) -> Result<u32, String> {
    let scale = value
        .parse::<u32>()
        .map_err(|_| format!("Invalid scale value: {value:?}"))?;

    if scale < 1 {
        return Err(format!(
            "Scale must be a positive integer (>= 1), got {value}"
        ));
    }

    Ok(scale)
}

/// Parse and validate a `--dim` value: it must be a (signed) integer.
///
/// Whether the integer names a supported dimension is checked separately by
/// [`dimension_info`], which also reports the set of valid ids.
fn parse_dim(value: &str) -> Result<i32, String> {
    value
        .parse::<i32>()
        .map_err(|_| format!("Invalid dimension id: {value:?}"))
}

/// Which non-overworld dimension a chunk's blocks tell us it belongs to.
///
/// Only used to disambiguate the legacy `DIM1` / `DIM-1` folders, whose names
/// do not reliably indicate which dimension they actually hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DimKind {
    Nether,
    End,
}

/// A supported dimension and where its region files live on disk.
#[derive(Clone, Copy, Debug)]
struct DimensionInfo {
    /// Human-readable name, e.g. "overworld".
    name: &'static str,
    /// What non-overworld dimension this is, used to match legacy region
    /// folders by their actual block content. `None` for the overworld.
    kind: Option<DimKind>,
    /// Candidate region folders (relative to the world folder), in order of
    /// preference.
    ///
    /// The modern `dimensions/minecraft:<name>/region` folders are named after
    /// the dimension, so an existing one is unambiguously the right one. The
    /// legacy `DIM1/region` and `DIM-1/region` folders are listed for both
    /// dimensions and disambiguated by content (see [`dimension_region_path`]).
    region_candidates: &'static [&'static str],
    /// Prefix applied to per-chunk output file names so that different
    /// dimensions do not overwrite one another. Empty for the default
    /// overworld, whose output names are left unchanged.
    out_prefix: &'static str,
    /// File name of the single-image output in `--single` mode.
    out_file: &'static str,
}

/// Map a dimension id to its on-disk region folder and output naming.
///
/// Dimension ids follow Minecraft's classic convention:
///
/// * `0`  -> overworld (`region/`)
/// * `1`  -> the nether
/// * `-1` -> the end
///
/// Any other id is rejected with a message listing the valid values.
fn dimension_info(id: i32) -> Result<DimensionInfo, String> {
    match id {
        0 => Ok(DimensionInfo {
            name: "overworld",
            kind: None,
            region_candidates: &["region"],
            out_prefix: "",
            out_file: "world.png",
        }),
        1 => Ok(DimensionInfo {
            name: "the_nether",
            kind: Some(DimKind::Nether),
            region_candidates: &[
                "dimensions/minecraft:the_nether/region",
                "DIM1/region",
                "DIM-1/region",
            ],
            out_prefix: "the_nether_",
            out_file: "the_nether.png",
        }),
        -1 => Ok(DimensionInfo {
            name: "the_end",
            kind: Some(DimKind::End),
            region_candidates: &[
                "dimensions/minecraft:the_end/region",
                "DIM1/region",
                "DIM-1/region",
            ],
            out_prefix: "the_end_",
            out_file: "the_end.png",
        }),
        other => Err(format!(
            "Unknown dimension id {other}. Valid ids: 0 (overworld), 1 (the nether), -1 (the end)."
        )),
    }
}

/// Block ids that are a reliable signature of each non-overworld dimension.
const NETHER_BLOCKS: &[&str] = &[
    "minecraft:netherrack",
    "minecraft:nether_wart_block",
    "minecraft:nether_bricks",
    "minecraft:soul_sand",
    "minecraft:soul_soil",
    "minecraft:crimson_roots",
    "minecraft:warped_roots",
    "minecraft:crimson_stem",
    "minecraft:warped_stem",
    "minecraft:magma_block",
    "minecraft:crimson_fungus",
    "minecraft:warped_fungus",
    "minecraft:weeping_vines",
    "minecraft:twisting_vines",
    "minecraft:shroomlight",
    "minecraft:red_mushroom",
    "minecraft:brown_mushroom",
];
const END_BLOCKS: &[&str] = &[
    "minecraft:end_stone",
    "minecraft:ender_chest",
    "minecraft:end_rod",
];

/// Classify a region folder by the dimension its chunks actually contain.
///
/// Samples the top-most block of as many chunk columns as needed until a clear
/// run of dimension signature blocks shows up. Returns `None` when the folder
/// holds no recognizable signature (e.g. it is empty).
fn classify_region_dir(region_dir: &Path) -> Option<DimKind> {
    let mut nether = 0u32;
    let mut end = 0u32;
    let mut files: Vec<PathBuf> = fs::read_dir(region_dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "mca"))
        .collect();
    files.sort();

    for path in &files {
        let Ok(data) = fs::read(path) else { continue };
        let Ok(mut region) = RegionReader::new(&data) else { continue };
        'chunks: for local_z in 0..32u8 {
            for local_x in 0..32u8 {
                let Ok(Some(chunk_data)) = region.chunk(local_x, local_z) else {
                    continue;
                };
                let Ok(chunk) = from_bytes::<Chunk>(&chunk_data) else {
                    continue;
                };
                for block in get_top_blocks(&chunk).blocks.iter().flatten() {
                    if NETHER_BLOCKS.contains(&block.as_str()) {
                        nether += 1;
                    } else if END_BLOCKS.contains(&block.as_str()) {
                        end += 1;
                    }
                }
                if nether.saturating_add(end) >= 16 {
                    break 'chunks;
                }
            }
        }
    }

    if nether == 0 && end == 0 {
        None
    } else if nether > end {
        Some(DimKind::Nether)
    } else {
        Some(DimKind::End)
    }
}

/// Locate a dimension's on-disk region folder.
///
/// Modern `dimensions/minecraft:<name>/region` folders are named after the
/// dimension, so the first one that exists is used as-is. The legacy
/// `DIM1/region` and `DIM-1/region` folders are ambiguous (different worlds
/// store the nether and the end in different one of them), so each existing
/// legacy folder is inspected by its block content and only the one matching
/// [`DimensionInfo::kind`] is used. Returns `None` if no matching folder is
/// found.
fn dimension_region_path(world_path: &Path, dim: &DimensionInfo) -> Option<PathBuf> {
    // 1) The overworld never needs disambiguation.
    let Some(kind) = dim.kind else {
        return dim
            .region_candidates
            .iter()
            .map(|cand| world_path.join(*cand))
            .find(|p| p.is_dir());
    };

    // 2) Modern, dimension-named folders are unambiguous.
    for cand in dim.region_candidates {
        if cand.starts_with("dimensions/minecraft:") {
            let path = world_path.join(*cand);
            if path.is_dir() {
                return Some(path);
            }
        }
    }

    // 3) Legacy folders: pick the one whose content matches this dimension.
    for cand in dim.region_candidates {
        if cand.starts_with("dimensions/minecraft:") {
            continue;
        }
        let path = world_path.join(*cand);
        if path.is_dir() && classify_region_dir(&path) == Some(kind) {
            return Some(path);
        }
    }

    None
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            print_usage();
            std::process::exit(1);
        }
    };

    let dim = match dimension_info(args.dim) {
        Ok(dim) => dim,
        Err(e) => {
            eprintln!("{e}");
            print_usage();
            std::process::exit(1);
        }
    };

    let region_path = match dimension_region_path(Path::new(&args.world_path), &dim) {
        Some(path) => path,
        None => {
            let tried = dim
                .region_candidates
                .iter()
                .map(|cand| {
                    Path::new(&args.world_path)
                        .join(*cand)
                        .display()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "Region directory does not exist for {}. Looked for: {tried}",
                dim.name
            )
            .into());
        }
    };

    let mut region_files = Vec::<PathBuf>::new();

    for entry in fs::read_dir(&region_path)? {
        let path = entry?.path();

        if path.extension().is_some_and(|ext| ext == "mca") {
            region_files.push(path);
        }
    }

    region_files.sort();

    let out_dir = Path::new("output");
    fs::create_dir_all(out_dir)?;

    // First pass: count the chunks that actually exist so we can build a
    // determinate progress bar, and track the world's bounding box so we can
    // size the single output image in --single mode.
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

    if total_chunks == 0 {
        println!("No chunks found in {}", region_path.display());
        return Ok(());
    }

    let pb = ProgressBar::new(total_chunks as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos:>7}/{len:7} {msg}",
        )
        .expect("valid progress bar template"),
    );

    if args.single {
        let bounds = world_bounds.expect("bounds must exist when chunks exist");

        let scale = args.scale as i32;
        let ss = if args.supersample { SUPER_SAMPLE as i32 } else { 1 };
        let blocks_w = (bounds.max_x - bounds.min_x + 1) * (CHUNK_SIZE as i32);
        let blocks_h = (bounds.max_z - bounds.min_z + 1) * (CHUNK_SIZE as i32);

        // In supersample mode each block becomes a solid `ss x ss` pixel square
        // (upsampling), so the image is larger by that factor. Otherwise each
        // output pixel covers `scale x scale` blocks (downsampling), so the
        // image is smaller by that factor (rounded up to a whole pixel).
        let width = if args.supersample {
            blocks_w * ss
        } else {
            (blocks_w + scale - 1) / scale
        };
        let height = if args.supersample {
            blocks_h * ss
        } else {
            (blocks_h + scale - 1) / scale
        };
        let (width, height) = (width as u32, height as u32);

        let res_desc = if args.supersample {
            format!("supersampled {} px/block", SUPER_SAMPLE)
        } else {
            format!("scale {}", args.scale)
        };
        let shadows_note = if args.shadows { " with shadows" } else { "" };
        println!(
            "Found {} region files and {} chunks in {} ({}). Rendering a single {}x{} PNG ({res_desc}){shadows_note} to {}",
            region_files.len(),
            total_chunks,
            region_path.display(),
            dim.name,
            width,
            height,
            out_dir.display()
        );

        let mut img = RgbImage::new(width, height);

        if args.shadows {
            // Shadow rendering is done over the whole map:
            //
            //   1. One pass over the region files fills two global grids -- the
            //      surface height and the base top-down color of every block
            //      column inside the world bounding box.
            //   2. A single scan works out, for every column, whether a 45
            //      degree sun sitting at the top-right is blocked by something
            //      up-right of it, i.e. whether the column is in shadow.
            //   3. The image is filled in-memory from those grids, darkening
            //      shadowed columns. No second region read is needed.
            //
            // In supersample mode the shadow is resolved at pixel resolution
            // (see `supersampled_heights`) so its diagonal edges stay smooth
            // even though the block colors are drawn as solid squares.
            let grid_w = blocks_w as usize;
            let grid_h = blocks_h as usize;
            let n = grid_w * grid_h;

            let mut heights = vec![VOID_H; n];
            let mut colors = vec![NO_BLOCK; n];

            for path in &region_files {
                collect_grid(
                    path,
                    &mut heights,
                    &mut colors,
                    grid_w,
                    bounds.min_x,
                    bounds.min_z,
                    &pb,
                )?;
            }

            if args.supersample {
                let s = SUPER_SAMPLE as usize;
                let pw = grid_w * s;
                let ph = grid_h * s;
                let pixel_heights = supersampled_heights(&heights, grid_w, grid_h, s);
                let pixel_shadow = compute_shadows(&pixel_heights, pw, ph);
                drop(pixel_heights);
                drop(heights);

                render_shaded_map_ss(&mut img, &colors, &pixel_shadow, grid_w, grid_h, s);
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
                    args.scale,
                );
            }
        } else if args.supersample {
            let s = SUPER_SAMPLE as i32;
            for path in &region_files {
                process_region_single_ss(
                    path,
                    &mut img,
                    bounds.min_x,
                    bounds.min_z,
                    s,
                    &pb,
                )?;
            }
        } else {
            for path in &region_files {
                process_region_single(
                    path,
                    &mut img,
                    bounds.min_x,
                    bounds.min_z,
                    args.scale,
                    &pb,
                )?;
            }
        }

        pb.finish_with_message("Done");

        let out_path = out_dir.join(dim.out_file);
        img.save(&out_path)?;

        println!();
        println!("Generated chunks: {total_chunks}");
        println!("Output: {}", out_path.display());
        return Ok(());
    }

    if args.shadows {
        println!("Note: -r/--shadows only applies in single mode (-s); ignoring it here.");
    }

    println!(
        "Found {} region files and {} chunks in {} ({}). Writing PNGs to {}",
        region_files.len(),
        total_chunks,
        region_path.display(),
        dim.name,
        out_dir.display()
    );

    for path in &region_files {
        process_region(path, out_dir, args.scale, args.supersample, dim.out_prefix, &pb)?;
    }

    pb.finish_with_message("Done");

    println!();
    println!("Generated chunks: {total_chunks}");
    println!("Output directory: {}", out_dir.display());

    Ok(())
}

fn process_region(
    path: &Path,
    out_dir: &Path,
    scale: u32,
    supersample: bool,
    dim_prefix: &str,
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

            let out_path =
                out_dir.join(format!("{dim_prefix}c_{chunk_x}_{chunk_z}.png"));
            if supersample {
                render_chunk_png_ss(&top, &out_path, SUPER_SAMPLE)?;
            } else {
                render_chunk_png(&top, &out_path, scale)?;
            }

            generated += 1;
            pb.inc(1);
        }
    }

    Ok(generated)
}

/// Best-effort scan of a region file.
///
/// Counts the chunks that actually exist and tracks the min/max chunk
/// coordinates across them. The count drives the determinate progress bar, and
/// the bounding box is used to size the single output image in `--single` mode.
/// Errors are swallowed so a single bad file does not abort the scan pass (the
/// processing pass will skip the same file).
///
/// Returns `(chunk_count, bounds)`; `bounds` is `None` when the region holds
/// no chunks.
fn scan_region(path: &Path) -> (usize, Option<Bounds>) {
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

/// Parse the `r.X.Z.mca` filename into (region_x, region_z).
fn parse_region_coords(path: &Path) -> Option<(i32, i32)> {
    let filename = path.file_stem()?.to_str()?;

    let parts: Vec<&str> = filename.split('.').collect();

    if parts.len() != 3 || parts[0] != "r" {
        return None;
    }

    let x: i32 = parts[1].parse().ok()?;
    let z: i32 = parts[2].parse().ok()?;

    Some((x, z))
}

/// Render the top-down view of a chunk as a PNG.
///
/// At `scale = 1` the output is 16x16 (1 pixel = 1 block) and each pixel is
/// colored by the highest non-air block in that column. At `scale = N` the
/// output is `ceil(16 / N)` on each side, and every pixel is colored by the
/// most common block in its `N x N` area.
fn render_chunk_png(
    top: &ChunkTop,
    out_path: &Path,
    scale: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let size = (CHUNK_SIZE + scale as usize - 1) / scale as usize;

    let mut img = RgbImage::new(size as u32, size as u32);

    blit_scaled(&mut img, &top.blocks, CHUNK_SIZE, CHUNK_SIZE, scale, 0, 0);

    img.save(out_path)?;

    Ok(())
}

/// Blit a chunk into an already-sized shared image at a given pixel offset.
///
/// `offset_x` / `offset_z` are the pixel coordinates of the chunk's top-left
/// corner inside the big image. At `scale = 1` that is the chunk's block
/// offset; at `scale = N` it is the chunk's block offset divided by N, and the
/// chunk is downsampled so each pixel represents an `N x N` block area.
fn render_into_big(
    img: &mut RgbImage,
    top: &ChunkTop,
    offset_x: u32,
    offset_z: u32,
    scale: u32,
) {
    blit_scaled(img, &top.blocks, CHUNK_SIZE, CHUNK_SIZE, scale, offset_x, offset_z);
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
    region_w: usize,
    region_h: usize,
    scale: u32,
    origin_x: u32,
    origin_z: u32,
) {
    let scale = scale.max(1) as usize;

    // Fast path: no downsampling, one pixel per block.
    if scale == 1 {
        for z in 0..region_h {
            for x in 0..region_w {
                let rgb = match &blocks[z * region_w + x] {
                    Some(name) => block_color(name),
                    None => NO_BLOCK,
                };
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

            let rgb = most_common_color(blocks, region_w, x0, x1, z0, z1);

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
fn blit_supersampled(
    out: &mut RgbImage,
    blocks: &[Option<String>],
    region_w: usize,
    region_h: usize,
    s: usize,
    origin_x: u32,
    origin_z: u32,
) {
    let s = s.max(1);
    for z in 0..region_h {
        for x in 0..region_w {
            let rgb = match &blocks[z * region_w + x] {
                Some(name) => block_color(name),
                None => NO_BLOCK,
            };
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
fn render_chunk_png_ss(
    top: &ChunkTop,
    out_path: &Path,
    scale: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let size = CHUNK_SIZE * scale as usize;
    let mut img = RgbImage::new(size as u32, size as u32);
    blit_supersampled(&mut img, &top.blocks, CHUNK_SIZE, CHUNK_SIZE, scale as usize, 0, 0);
    img.save(out_path)?;

    Ok(())
}

/// Color of the most common block within the block rectangle
/// `[x0, x1) x [z0, z1)`.
///
/// Ties resolve to whichever block reaches the leading count first in
/// row-major scan order, so the result is deterministic. Returns `NO_BLOCK`
/// for an all-void area.
fn most_common_color(
    blocks: &[Option<String>],
    region_w: usize,
    x0: usize,
    x1: usize,
    z0: usize,
    z1: usize,
) -> [u8; 3] {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    let mut best_name: Option<&str> = None;
    let mut best_count: u32 = 0;

    for z in z0..z1 {
        for x in x0..x1 {
            let Some(name) = blocks[z * region_w + x].as_deref() else {
                continue;
            };

            let count = counts.entry(name).or_insert(0);
            *count += 1;

            if *count > best_count {
                best_count = *count;
                best_name = Some(name);
            }
        }
    }

    match best_name {
        Some(name) => block_color(name),
        None => NO_BLOCK,
    }
}

/// Darken a color to its "in shadow" appearance by scaling every channel by
/// [`SHADOW_NUMERATOR`] / [`SHADOW_DENOMINATOR`].
fn shade(rgb: [u8; 3]) -> [u8; 3] {
    [
        ((rgb[0] as u32 * SHADOW_NUMERATOR) / SHADOW_DENOMINATOR) as u8,
        ((rgb[1] as u32 * SHADOW_NUMERATOR) / SHADOW_DENOMINATOR) as u8,
        ((rgb[2] as u32 * SHADOW_NUMERATOR) / SHADOW_DENOMINATOR) as u8,
    ]
}

/// Compute a per-column shadow mask for a top-down map lit by a 45 degree sun
/// located at the top-right.
///
/// `heights` is a row-major grid of `grid_w x grid_h` column surface heights
/// (index = `z * grid_w + x`), where [`VOID_H`] marks a column with no block.
/// The result is a same-sized mask that is `true` where the column is in
/// shadow.
///
/// A 45 degree sun at the top-right sends light down toward the bottom-left,
/// so a column throws a diagonal shadow onto the columns below and to its
/// left. Because the light arrives at 45 degrees, the horizontal reach of the
/// shadow equals the height difference: a pillar that stands `N` blocks above
/// its surroundings casts a diagonal shadow `N` cells long.
///
/// Formally, column `(x, z)` is shadowed when some column further up its
/// diagonal -- `(x + t, z - t)` for `t >= 1` -- rises at least to the height
/// the light ray would be at that point, i.e. `h(x + t, z - t) >= h(x, z) + t`.
fn compute_shadows(heights: &[i32], grid_w: usize, grid_h: usize) -> Vec<bool> {
    let n = grid_w * grid_h;
    let mut shadow = vec![false; n];
    // `best[i]` = max over t >= 0 of (height of the diagonal cell t steps
    // up-right, minus t). It satisfies the recurrence
    //     best[i] = max(heights[i], best[up_right(i)] - 1)
    // and is computed scanning from the top-right toward the bottom-left so
    // the up-right dependency is already resolved when we reach `i`.
    let mut best = vec![VOID_H; n];

    for z in 0..grid_h {
        for x in (0..grid_w).rev() {
            let i = z * grid_w + x;
            let h = heights[i];

            // The up-right neighbour of (x, z) is (x + 1, z - 1).
            let up_val = if x + 1 < grid_w && z >= 1 {
                best[(z - 1) * grid_w + (x + 1)] - 1
            } else {
                VOID_H - 1
            };

            best[i] = if up_val > h { up_val } else { h };

            // A real (non-void) column is in shadow when something up-right of
            // it reaches at least as high as the ray would be at its position.
            if h != VOID_H && up_val >= h {
                shadow[i] = true;
            }
        }
    }

    shadow
}

/// Upsample a block-resolution height grid to pixel resolution by nearest
/// neighbor, so the shadow can be computed at pixel resolution in
/// `--supersample` mode.
///
/// Real columns are multiplied by `s` while void columns keep the [`VOID_H`]
/// sentinel. Multiplying the heights (rather than only the grid size) keeps the
/// 45 degree sun's geometry intact when [`compute_shadows`] runs on the finer
/// grid: a ray that descends one block per block still descends one block per
/// `s` pixels, so shadow lengths are unchanged -- just resolved at pixel
/// resolution, which makes the diagonal edges smooth.
fn supersampled_heights(
    heights: &[i32],
    grid_w: usize,
    grid_h: usize,
    s: usize,
) -> Vec<i32> {
    let pw = grid_w * s;
    let ph = grid_h * s;
    let mut out = vec![VOID_H; pw * ph];

    for z in 0..grid_h {
        for x in 0..grid_w {
            let bh = heights[z * grid_w + x];
            let phv = if bh == VOID_H { VOID_H } else { bh * s as i32 };
            for dz in 0..s {
                for dx in 0..s {
                    out[(z * s + dz) * pw + (x * s + dx)] = phv;
                }
            }
        }
    }

    out
}

/// First shadow pass: read every region once and fill the global height and
/// base-color grids for the whole world bounding box.
///
/// `heights` and `colors` are pre-sized to `grid_w * grid_h` and indexed as
/// `z * grid_w + x`, where (x, z) are block offsets from
/// (`min_chunk_x * 16`, `min_chunk_z * 16`). Columns are written to their
/// absolute position, so the region iteration order does not matter.
fn collect_grid(
    path: &Path,
    heights: &mut [i32],
    colors: &mut [[u8; 3]],
    grid_w: usize,
    min_chunk_x: i32,
    min_chunk_z: i32,
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

            let block_x0 = (chunk_x - min_chunk_x) * (CHUNK_SIZE as i32);
            let block_z0 = (chunk_z - min_chunk_z) * (CHUNK_SIZE as i32);

            for lz in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let gx = (block_x0 + lx as i32) as usize;
                    let gz = (block_z0 + lz as i32) as usize;
                    let gi = gz * grid_w + gx;

                    match &top.blocks[lz * CHUNK_SIZE + lx] {
                        Some(name) => {
                            heights[gi] =
                                top.heights[lz * CHUNK_SIZE + lx].unwrap_or(VOID_H);
                            colors[gi] = block_color(name);
                        }
                        None => {
                            heights[gi] = VOID_H;
                            colors[gi] = NO_BLOCK;
                        }
                    }
                }
            }

            pb.inc(1);
        }
    }

    Ok(())
}

/// Final shadow pass: fill `img` from the global `colors` + `shadow` grids,
/// darkening shadowed columns.
///
/// Every chunk in the world bounding box is blitted at the same pixel offset
/// the non-shadow single-mode pass uses, so the two modes share an identical
/// layout. Chunks that do not exist are all void in the grids and therefore
/// blit as black -- exactly the image's initial state.
fn render_shaded_map(
    img: &mut RgbImage,
    colors: &[[u8; 3]],
    shadow: &[bool],
    grid_w: usize,
    min_chunk_x: i32,
    min_chunk_z: i32,
    max_chunk_x: i32,
    max_chunk_z: i32,
    scale: u32,
) {
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
                    local.push(if shadow[gi] { shade(c) } else { c });
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

/// Supersample shadow pass: fill `img` (sized `grid_w * s x grid_h * s`) from
/// the block-resolution `colors` grid and a pixel-resolution `pixel_shadow`
/// mask.
///
/// Every block is drawn as a solid `s x s` pixel square of its base color
/// (no interpolation between blocks), but each of its pixels is darkened
/// independently according to `pixel_shadow`. Because the shadow was computed
/// at pixel resolution (see [`supersampled_heights`]), a block straddling the
/// shadow edge gets a mix of lit and dark pixels, which renders the diagonal
/// shadow boundary smoothly instead of as a coarse stairstep of blocks.
fn render_shaded_map_ss(
    img: &mut RgbImage,
    colors: &[[u8; 3]],
    pixel_shadow: &[bool],
    grid_w: usize,
    grid_h: usize,
    s: usize,
) {
    let pw = grid_w * s;

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
                    let rgb = if pixel_shadow[pi] { shade(c) } else { c };
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

/// Decode a region and composite each of its chunks into a shared big image.
///
/// `min_chunk_x` / `min_chunk_z` are the world's minimum chunk coordinates,
/// used to translate each chunk's absolute position into image pixel offsets.
/// At `scale = 1` the offset is the chunk's block offset; at `scale = N` it is
/// divided by N because the shared image is downsampled by that factor.
fn process_region_single(
    path: &Path,
    img: &mut RgbImage,
    min_chunk_x: i32,
    min_chunk_z: i32,
    scale: u32,
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

            render_into_big(img, &top, offset_x as u32, offset_z as u32, scale);

            pb.inc(1);
        }
    }

    Ok(())
}

/// Decode a region and composite each of its chunks into a shared big image in
/// supersample mode: every block becomes a solid `ss x ss` pixel square, so a
/// chunk sits at pixel offset `(chunk - min) * CHUNK_SIZE * ss` in the image.
fn process_region_single_ss(
    path: &Path,
    img: &mut RgbImage,
    min_chunk_x: i32,
    min_chunk_z: i32,
    ss: i32,
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
                CHUNK_SIZE,
                CHUNK_SIZE,
                ss as usize,
                offset_x,
                offset_z,
            );

            pb.inc(1);
        }
    }

    Ok(())
}



/// Extract the highest non-air block from every x/z column.
///
/// The output contains 256 entries:
///
///     index = z * 16 + x
///
/// Sections are processed from highest to lowest. Within each section,
/// block indices are processed from highest Y to lowest Y.
///
/// This avoids decoding the entire chunk into a 3D array.
fn get_top_blocks(chunk: &Chunk) -> ChunkTop {
    let mut blocks: [Option<String>; COLUMNS] = std::array::from_fn(|_| None);
    let mut heights: [Option<i32>; COLUMNS] = [None; COLUMNS];

    // Highest section first.
    let mut sections: Vec<&Section> = chunk.sections.iter().collect();

    sections.sort_unstable_by(|a, b| b.y.cmp(&a.y));

    for section in sections {
        let Some(block_states) = &section.block_states else {
            continue;
        };

        if block_states.palette.is_empty() {
            continue;
        }

        // If there is only one palette entry and no data array,
        // every block in this section is that palette entry.
        if block_states.data.is_none() {
            let entry = &block_states.palette[0];

            if is_air(&entry.name) {
                continue;
            }

            for z in 0..16 {
                for x in 0..16 {
                    let i = z * 16 + x;

                    if blocks[i].is_none() {
                        blocks[i] = Some(entry.name.clone());
                        heights[i] = Some(section.y as i32 * 16 + 15);
                    }
                }
            }

            continue;
        }

        let data = block_states.data.as_ref().unwrap();

        let bits_per_block = bits_per_block(block_states.palette.len());

        let entries_per_long = 64 / bits_per_block;

        let mask = (1u64 << bits_per_block) - 1;

        // Minecraft's section block index is:
        //
        //     y * 256 + z * 16 + x
        //
        // Iterate from highest Y downward.
        for local_y in (0..16).rev() {
            for z in 0..16 {
                for x in 0..16 {
                    let column = z * 16 + x;

                    // Already found a higher block in this column.
                    if blocks[column].is_some() {
                        continue;
                    }

                    let index = local_y * 256 + z * 16 + x;

                    let palette_index = read_packed_index(
                        data,
                        index,
                        bits_per_block,
                        entries_per_long,
                        mask,
                    );

                    let Some(entry) = block_states.palette.get(palette_index) else {
                        continue;
                    };

                    if is_air(&entry.name) {
                        continue;
                    }

                    blocks[column] = Some(entry.name.clone());

                    heights[column] =
                        Some(section.y as i32 * 16 + local_y as i32);
                }
            }
        }
    }

    ChunkTop { blocks, heights }
}

/// Number of bits required by a paletted container.
///
/// Minecraft uses at least 4 bits per block for block states.
fn bits_per_block(palette_size: usize) -> usize {
    if palette_size <= 1 {
        return 4;
    }

    let bits = (usize::BITS - (palette_size - 1).leading_zeros()) as usize;

    bits.max(4)
}

/// Read one packed palette index from the LongArray.
#[inline]
fn read_packed_index(
    data: &[i64],
    index: usize,
    bits: usize,
    entries_per_long: usize,
    mask: u64,
) -> usize {
    let long_index = index / entries_per_long;
    let index_inside_long = index % entries_per_long;

    let bit_offset = index_inside_long * bits;

    let value = data[long_index] as u64;

    ((value >> bit_offset) & mask) as usize
}


/// Blocks which don't count as the surface.
///
/// You can change this later depending on how you want your map
/// to behave.
fn is_air(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
    )
}

/// Map a Minecraft block name to a top-down view color.
///
/// Well-known surface blocks get hand-picked colors; anything else falls back
/// to a stable color derived from a hash of the name so distinct unknown
/// blocks are still visually distinguishable.
fn block_color(name: &str) -> [u8; 3] {
    match name {
        // Grass & dirt.
        "minecraft:grass_block" => [106, 170, 64],
        "minecraft:dirt"
        | "minecraft:dirt_with_roots"
        | "minecraft:coarse_dirt" => [134, 96, 67],
        "minecraft:rooted_dirt" => [124, 90, 60],
        "minecraft:podzol" => [95, 70, 42],
        "minecraft:mycelium" => [120, 104, 120],
        "minecraft:moss_block" => [80, 130, 60],

        // Stone family.
        "minecraft:stone" | "minecraft:stone_bricks" => [125, 125, 125],
        "minecraft:deepslate" | "minecraft:cobbled_deepslate" => [67, 67, 67],
        "minecraft:granite" | "minecraft:polished_granite" => [138, 73, 56],
        "minecraft:diorite" | "minecraft:polished_diorite" => [200, 200, 200],
        "minecraft:andesite" | "minecraft:polished_andesite" => [136, 136, 136],
        "minecraft:tuff" => [95, 95, 95],
        "minecraft:calcite" => [233, 231, 226],
        "minecraft:dripstone_block" => [170, 145, 120],
        "minecraft:basalt" | "minecraft:polished_basalt" => [110, 108, 108],
        "minecraft:bedrock" => [110, 110, 110],
        "minecraft:obsidian" => [28, 24, 38],
        "minecraft:crying_obsidian" => [30, 30, 60],
        "minecraft:bricks" => [150, 96, 84],

        // Nether.
        "minecraft:netherrack" => [135, 58, 52],
        "minecraft:nether_bricks" | "minecraft:red_nether_bricks" => [45, 30, 30],
        "minecraft:nether_wart_block" => [96, 32, 110],
        "minecraft:soul_sand" | "minecraft:soul_soil" => [120, 110, 105],
        "minecraft:quartz_block" => [234, 228, 221],
        "minecraft:blackstone" => [42, 42, 42],
        "minecraft:magma_block" => [190, 60, 30],

        // The End.
        "minecraft:end_stone" | "minecraft:end_stone_bricks" => [221, 219, 165],
        "minecraft:purpur_block" => [197, 168, 220],

        // Sand & stone variants.
        "minecraft:sand" => [219, 209, 160],
        "minecraft:red_sand" => [190, 110, 70],
        "minecraft:sandstone" => [217, 209, 158],
        "minecraft:red_sandstone" => [189, 110, 60],

        // Snow & ice.
        "minecraft:snow" | "minecraft:snow_block" | "minecraft:powder_snow" => {
            [247, 250, 253]
        }
        "minecraft:packed_ice" => [145, 190, 231],
        "minecraft:ice" => [126, 175, 232],
        "minecraft:blue_ice" => [98, 162, 232],

        // Water & lava.
        "minecraft:water" => [62, 121, 201],
        "minecraft:lava" => [243, 118, 53],

        // Wood: planks.
        "minecraft:oak_planks" => [162, 130, 78],
        "minecraft:spruce_planks" => [112, 84, 50],
        "minecraft:birch_planks" => [192, 175, 121],
        "minecraft:jungle_planks" => [160, 134, 68],
        "minecraft:acacia_planks" => [168, 121, 53],
        "minecraft:dark_oak_planks" => [66, 43, 20],
        "minecraft:mangrove_planks" => [145, 104, 86],
        "minecraft:cherry_planks" => [185, 143, 133],
        "minecraft:pale_oak_planks" => [190, 178, 124],

        // Wood: logs.
        "minecraft:oak_log" => [104, 82, 50],
        "minecraft:spruce_log" => [55, 40, 25],
        "minecraft:birch_log" => [226, 224, 216],
        "minecraft:jungle_log" => [134, 114, 62],
        "minecraft:acacia_log" => [148, 103, 62],
        "minecraft:dark_oak_log" => [46, 35, 16],
        "minecraft:mangrove_log" => [120, 90, 70],
        "minecraft:cherry_log" => [150, 105, 95],
        "minecraft:pale_oak_log" => [180, 165, 120],

        // Misc.
        "minecraft:glass" => [190, 224, 235],
        "minecraft:glowstone" => [250, 218, 138],
        "minecraft:terracotta" => [152, 92, 69],
        "minecraft:clay" => [160, 170, 190],
        "minecraft:bone_block" => [230, 228, 210],
        "minecraft:slime" => [96, 146, 60],

        // Anything not listed above gets a stable hash-based color.
        _ => hash_color(name),
    }
}

/// Stable pseudo-random color derived from the block name (FNV-1a).
///
/// Keeps unknown blocks visually distinct from one another and stable across
/// runs.
fn hash_color(name: &str) -> [u8; 3] {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;

    for b in name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }

    [
        (30 + (h % 180)) as u8,
        (30 + ((h >> 8) % 180)) as u8,
        (30 + ((h >> 16) % 180)) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    #[test]
    fn parses_region_coords() {
        assert_eq!(
            parse_region_coords(Path::new("region/r.1.-2.mca")),
            Some((1, -2))
        );
        assert_eq!(
            parse_region_coords(Path::new("region/r.0.0.mca")),
            Some((0, 0))
        );
        // Not an r.X.Z file.
        assert_eq!(parse_region_coords(Path::new("region/foo.mca")), None);
        // Too few components.
        assert_eq!(parse_region_coords(Path::new("region/r.1.mca")), None);
        // Non-numeric component.
        assert_eq!(
            parse_region_coords(Path::new("region/r.x.z.mca")),
            None
        );
    }

    #[test]
    fn known_blocks_get_their_colors() {
        assert_eq!(block_color("minecraft:grass_block"), [106, 170, 64]);
        assert_eq!(block_color("minecraft:water"), [62, 121, 201]);
        assert_eq!(block_color("minecraft:sand"), [219, 209, 160]);
    }

    #[test]
    fn unknown_blocks_are_stable_and_bounded() {
        let a = hash_color("minecraft:some_unknown_block");
        let b = hash_color("minecraft:some_unknown_block");
        assert_eq!(a, b, "hash color must be deterministic");

        for channel in a {
            assert!((30..=210).contains(&channel));
        }

        assert_ne!(
            hash_color("minecraft:one_block"),
            hash_color("minecraft:two_block"),
            "distinct blocks should get distinct fallback colors"
        );
    }

    #[test]
    fn renders_a_16x16_png_with_expected_pixels() {
        // Only column (x=0, z=0) has a block; everything else is void.
        let blocks = std::array::from_fn(|i| {
            (i == 0).then(|| "minecraft:grass_block".to_string())
        });
        let top = ChunkTop {
            blocks,
            heights: [None; COLUMNS],
        };

        let path = std::env::temp_dir().join("worldraw_test_chunk.png");
        render_chunk_png(&top, &path, 1).expect("render should succeed");

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
        };
        let top_b = ChunkTop {
            blocks: std::array::from_fn(|_| Some("minecraft:sand".to_string())),
            heights: [None; COLUMNS],
        };

        let mut img = RgbImage::new(32, 16);
        render_into_big(&mut img, &top_a, 0, 0, 1);
        render_into_big(&mut img, &top_b, 16, 0, 1);

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
        };

        let path = std::env::temp_dir().join("worldraw_test_scaled.png");
        render_chunk_png(&top, &path, 2).expect("render should succeed");

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
        };

        let path = std::env::temp_dir().join("worldraw_test_scaled4.png");
        render_chunk_png(&top, &path, 4).expect("render should succeed");

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
    fn bounds_merge_covers_both_boxes() {
        let a = Bounds {
            min_x: -5,
            min_z: -10,
            max_x: 5,
            max_z: 10,
        };
        let b = Bounds {
            min_x: 0,
            min_z: -1,
            max_x: 3,
            max_z: 100,
        };
        let merged = a.merged(&b);
        assert_eq!(merged.min_x, -5);
        assert_eq!(merged.min_z, -10);
        assert_eq!(merged.max_x, 5);
        assert_eq!(merged.max_z, 100);
    }

    #[test]
    fn dimension_info_maps_known_ids() {
        let overworld = dimension_info(0).expect("overworld should be valid");
        assert_eq!(overworld.name, "overworld");
        assert_eq!(overworld.kind, None);
        assert_eq!(overworld.region_candidates, ["region"]);
        assert_eq!(overworld.out_prefix, "");
        assert_eq!(overworld.out_file, "world.png");

        let nether = dimension_info(1).expect("nether should be valid");
        assert_eq!(nether.name, "the_nether");
        assert_eq!(nether.kind, Some(DimKind::Nether));
        // The modern layout is named after the dimension and is unambiguous.
        assert!(nether
            .region_candidates
            .contains(&"dimensions/minecraft:the_nether/region"));
        // The legacy layout is shared and disambiguated by content, so both
        // folder names are offered as candidates.
        assert!(nether.region_candidates.contains(&"DIM1/region"));
        assert!(nether.region_candidates.contains(&"DIM-1/region"));
        assert_eq!(nether.out_prefix, "the_nether_");
        assert_eq!(nether.out_file, "the_nether.png");

        let end = dimension_info(-1).expect("the end should be valid");
        assert_eq!(end.name, "the_end");
        assert_eq!(end.kind, Some(DimKind::End));
        assert!(end
            .region_candidates
            .contains(&"dimensions/minecraft:the_end/region"));
        assert!(end.region_candidates.contains(&"DIM1/region"));
        assert!(end.region_candidates.contains(&"DIM-1/region"));
        assert_eq!(end.out_prefix, "the_end_");
        assert_eq!(end.out_file, "the_end.png");
    }

    #[test]
    fn dimension_info_rejects_unknown_ids() {
        assert!(dimension_info(2).is_err());
        assert!(dimension_info(-2).is_err());
        assert!(dimension_info(99).is_err());
    }

    #[test]
    fn parse_dim_validates_input() {
        assert_eq!(parse_dim("0"), Ok(0));
        assert_eq!(parse_dim("1"), Ok(1));
        assert_eq!(parse_dim("-1"), Ok(-1));
        assert!(parse_dim("abc").is_err());
        assert!(parse_dim("1.5").is_err());
    }

    #[test]
    fn flat_terrain_casts_no_shadow() {
        let grid_w = 8;
        let grid_h = 8;
        let heights = vec![5i32; grid_w * grid_h];
        let shadow = compute_shadows(&heights, grid_w, grid_h);
        assert!(shadow.iter().all(|&s| !s), "flat terrain has no shadow");
    }

    #[test]
    fn pillar_casts_a_diagonal_shadow_toward_bottom_left() {
        let grid_w = 8;
        let grid_h = 8;
        // Flat field at height 0 with a single pillar of height 3 at (x=4, z=1).
        let mut heights = vec![0i32; grid_w * grid_h];
        heights[1 * grid_w + 4] = 3;

        let shadow = compute_shadows(&heights, grid_w, grid_h);
        let idx = |x: usize, z: usize| z * grid_w + x;

        // The shadow runs diagonally toward the bottom-left, one cell per block
        // of height difference: exactly (3,2), (2,3) and (1,4).
        assert!(shadow[idx(3, 2)], "first shadow cell");
        assert!(shadow[idx(2, 3)], "second shadow cell");
        assert!(shadow[idx(1, 4)], "third shadow cell");
        // The fourth diagonal cell is beyond the 3-block reach and is lit.
        assert!(!shadow[idx(0, 5)], "beyond the shadow length");
        // The pillar top and its right / south sides are not shadowed.
        assert!(!shadow[idx(4, 1)], "pillar top is lit");
        assert!(!shadow[idx(4, 2)], "south of the pillar is lit");
        assert!(!shadow[idx(5, 1)], "right of the pillar is lit");
        // The rest of the flat field is lit.
        assert!(!shadow[idx(0, 0)]);
        assert!(!shadow[idx(7, 7)]);
    }

    #[test]
    fn shade_darkens_and_keeps_void_black() {
        assert_eq!(shade([100, 200, 50]), [55, 110, 27]);
        assert_eq!(shade(NO_BLOCK), NO_BLOCK);

        let c = [255u8, 200, 100];
        let s = shade(c);
        assert!(s[0] <= c[0] && s[1] <= c[1] && s[2] <= c[2]);
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
        render_shaded_map(&mut img, &colors, &shadow, grid_w, 0, 0, 0, 0, 1);

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
        };

        let path = std::env::temp_dir().join("worldraw_test_ss.png");
        render_chunk_png_ss(&top, &path, SUPER_SAMPLE).expect("render should succeed");

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
    fn supersampled_heights_scales_real_columns_and_keeps_void() {
        // 2x2 block grid: three flat (height 0) and one pillar of height 3.
        let mut heights = vec![0i32; 4];
        heights[1] = 3;
        let s = 5usize;

        let out = supersampled_heights(&heights, 2, 2, s);
        assert_eq!(out.len(), (2 * s) * (2 * s));

        // Block (0,0) (height 0) -> its 5x5 pixels are all 0.
        assert_eq!(out[(0 * s) * (2 * s) + 0 * s], 0);
        // Block (1,0) (height 3) -> its 5x5 pixels are all 3 * s = 15.
        assert_eq!(out[(0 * s) * (2 * s) + 1 * s], 15);

        // A void column keeps the sentinel.
        let void = vec![VOID_H, 0, 0, 0];
        let void_out = supersampled_heights(&void, 2, 2, s);
        assert_eq!(void_out[0], VOID_H);
    }

    #[test]
    fn supersample_shadow_is_smooth_at_pixel_resolution() {
        let grid_w = 8;
        let grid_h = 8;
        let s = 5usize;
        // Flat field at height 0 with a single pillar of height 3 at (x=4, z=1).
        let mut heights = vec![0i32; grid_w * grid_h];
        heights[1 * grid_w + 4] = 3;

        let pixel_heights = supersampled_heights(&heights, grid_w, grid_h, s);
        let pw = grid_w * s;
        let ph = grid_h * s;
        let shadow = compute_shadows(&pixel_heights, pw, ph);

        // A smooth, pixel-resolved shadow means some single block straddles the
        // edge: within its 5x5 pixels there is a mix of lit and shadowed pixels
        // (a block-resolved shadow would make each block uniformly lit or dark).
        let mut found_partial = false;
        for z in 0..grid_h {
            for x in 0..grid_w {
                let mut lit = 0;
                let mut dark = 0;
                for dz in 0..s {
                    for dx in 0..s {
                        let pi = (z * s + dz) * pw + (x * s + dx);
                        if shadow[pi] {
                            dark += 1;
                        } else {
                            lit += 1;
                        }
                    }
                }
                if lit > 0 && dark > 0 {
                    found_partial = true;
                }
            }
        }
        assert!(
            found_partial,
            "a block should straddle the smooth shadow edge at pixel resolution"
        );
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
        render_shaded_map_ss(&mut img, &colors, &pixel_shadow, grid_w, grid_h, s);

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
}

