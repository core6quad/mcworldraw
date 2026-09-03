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

    /// Absolute Minecraft Y coordinate.
    ///
    /// Retained for potential future elevation-based shading; not currently
    /// used by the renderer.
    #[allow(dead_code)]
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
         -h, --help           Show this message"
    );
}

fn parse_args() -> Result<Args, String> {
    let mut world_path: Option<String> = None;
    let mut single = false;
    let mut scale: u32 = 1;

    let raw: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;

    while i < raw.len() {
        let arg = raw[i].as_str();

        match arg {
            "--single" | "--big" | "-s" => single = true,
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
            }
            _ if arg.starts_with("--scale=") => {
                scale = parse_scale(&arg["--scale=".len()..])?;
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

    let world_path =
        world_path.ok_or_else(|| "Missing <path-to-world> argument".to_string())?;

    Ok(Args { world_path, single, scale })
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            print_usage();
            std::process::exit(1);
        }
    };

    let region_path = Path::new(&args.world_path).join("region");

    if !region_path.is_dir() {
        return Err(format!(
            "Region directory does not exist: {}",
            region_path.display()
        )
        .into());
    }

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
        let blocks_w = (bounds.max_x - bounds.min_x + 1) * (CHUNK_SIZE as i32);
        let blocks_h = (bounds.max_z - bounds.min_z + 1) * (CHUNK_SIZE as i32);

        // Each output pixel covers `scale x scale` blocks, so the image is
        // smaller by that factor (rounded up to a whole pixel).
        let width = (blocks_w + scale - 1) / scale;
        let height = (blocks_h + scale - 1) / scale;
        let (width, height) = (width as u32, height as u32);

        println!(
            "Found {} region files and {} chunks. Rendering a single {}x{} PNG (scale {}) to {}",
            region_files.len(),
            total_chunks,
            width,
            height,
            args.scale,
            out_dir.display()
        );

        let mut img = RgbImage::new(width, height);

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

        pb.finish_with_message("Done");

        let out_path = out_dir.join("world.png");
        img.save(&out_path)?;

        println!();
        println!("Generated chunks: {total_chunks}");
        println!("Output: {}", out_path.display());
        return Ok(());
    }

    println!(
        "Found {} region files and {} chunks. Writing PNGs to {}",
        region_files.len(),
        total_chunks,
        out_dir.display()
    );

    for path in &region_files {
        process_region(path, out_dir, args.scale, &pb)?;
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

            let out_path = out_dir.join(format!("c_{chunk_x}_{chunk_z}.png"));
            render_chunk_png(&top, &out_path, scale)?;

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
}

