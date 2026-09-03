use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use fastnbt::{from_bytes, LongArray};
use mca::RegionReader;
use serde::Deserialize;

const CHUNK_SIZE: usize = 16;
const COLUMNS: usize = 256;

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
    heights: [Option<i32>; COLUMNS],
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let world_path = env::args()
        .nth(1)
        .expect("Usage: mcmap <path-to-world>");

    let region_path = Path::new(&world_path).join("region");

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

    println!("Found {} region files", region_files.len());

    let mut total_chunks = 0usize;

    for path in region_files {
        total_chunks += process_region(&path)?;
    }

    println!();
    println!("Done.");
    println!("Generated chunks: {total_chunks}");

    Ok(())
}

fn process_region(path: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let filename = path
        .file_stem()
        .ok_or("Invalid region filename")?
        .to_string_lossy();

    // Expected:
    //
    // r.X.Z.mca
    //
    let parts: Vec<&str> = filename.split('.').collect();

    if parts.len() != 3 || parts[0] != "r" {
        eprintln!("Skipping invalid region filename: {}", path.display());
        return Ok(0);
    }

    let region_x: i32 = parts[1].parse()?;
    let region_z: i32 = parts[2].parse()?;

    println!(
        "Region ({region_x}, {region_z}) - {}",
        path.file_name().unwrap().to_string_lossy()
    );

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

            generated += 1;

            println!(
                "  chunk ({chunk_x:>7}, {chunk_z:>7}) -> {} sections",
                chunk.sections.len()
            );

            // Print the center column as a simple sanity check.
            //
            // x = 8
            // z = 8
            //
            let i = 8 * 16 + 8;

            match (&top.blocks[i], top.heights[i]) {
                (Some(block), Some(y)) => {
                    println!("      center: {block} at Y={y}");
                }

                _ => {
                    println!("      center: no solid block");
                }
            }
        }
    }

    Ok(generated)
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

