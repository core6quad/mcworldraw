//! NBT chunk data model and surface (top-block) extraction.
//!
//! This module owns the on-disk representation of a Minecraft chunk (the NBT
//! structs decoded by `fastnbt`) and the logic that reduces a full chunk to its
//! top-down surface information: for every x/z column, the highest non-air
//! block, its absolute Y (the column's surface height), and -- when the surface
//! is water -- the first solid block beneath it. It also defines the chunk
//! geometry constants shared across the crate.

use fastnbt::LongArray;
use serde::Deserialize;

/// Number of blocks on one side of a chunk (chunks are 16x16 columns of blocks).
pub(crate) const CHUNK_SIZE: usize = 16;

/// Number of x/z columns in a chunk (16 * 16).
pub(crate) const COLUMNS: usize = 256;

/// Sentinel surface height for a column that has no block (pure void).
///
/// Real column heights are actual Minecraft Y values, so a value far below any
/// possible Y marks "no block" without colliding with real terrain. Used by the
/// shadow pass so void columns neither cast nor receive a shadow.
pub(crate) const VOID_H: i32 = -1_000_000;

#[derive(Debug, Deserialize)]
pub(crate) struct Chunk {
    #[serde(default)]
    sections: Vec<Section>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Section {
    #[serde(rename = "Y")]
    y: i8,

    #[serde(default)]
    block_states: Option<BlockStates>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BlockStates {
    palette: Vec<PaletteEntry>,

    #[serde(default)]
    data: Option<LongArray>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PaletteEntry {
    #[serde(rename = "Name")]
    name: String,
}

/// The top block for every x/z column in a chunk.
///
/// Index = z * 16 + x
#[derive(Clone, Debug)]
pub(crate) struct ChunkTop {
    /// Minecraft block name, e.g. "minecraft:grass_block"
    pub(crate) blocks: [Option<String>; COLUMNS],

    /// Absolute Minecraft Y coordinate of the top block (the column's surface
    /// height), or `None` for a void column.
    ///
    /// Used by the shadow pass to compute elevation-based shadows.
    pub(crate) heights: [Option<i32>; COLUMNS],

    /// When the top block is water, the name of the first non-water, non-air
    /// block beneath it (the lake/sea floor seen through the water), or `None`
    /// if there is no such block or the surface is not water.
    ///
    /// Only populated to support `--transparency`, where a water column is
    /// blended with this block's color.
    pub(crate) under: [Option<String>; COLUMNS],
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
pub(crate) fn get_top_blocks(chunk: &Chunk) -> ChunkTop {
    let mut blocks: [Option<String>; COLUMNS] = std::array::from_fn(|_| None);
    let mut heights: [Option<i32>; COLUMNS] = [None; COLUMNS];
    let mut under: [Option<String>; COLUMNS] = std::array::from_fn(|_| None);

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

    // Second pass: for every column whose surface is water, find the first
    // non-water, non-air block beneath it so `--transparency` can blend the
    // water with the floor seen through it.
    for i in 0..COLUMNS {
        if let (Some(name), Some(h)) = (&blocks[i], heights[i]) {
            if is_water(name) {
                let x = i % CHUNK_SIZE;
                let z = i / CHUNK_SIZE;
                under[i] = find_block_under_water(chunk, x, z, h);
            }
        }
    }

    ChunkTop {
        blocks,
        heights,
        under,
    }
}

/// Read the name of a single block at `(local_y, x, z)` inside a section.
///
/// Returns `None` when the section has no block states or an empty palette.
/// Handles both the "single palette entry, no data" fast path and the packed
/// paletted container.
fn read_block_at(
    section: &Section,
    local_y: usize,
    x: usize,
    z: usize,
) -> Option<String> {
    let block_states = section.block_states.as_ref()?;
    if block_states.palette.is_empty() {
        return None;
    }

    let name = if block_states.data.is_none() {
        block_states.palette[0].name.clone()
    } else {
        let data = block_states.data.as_ref().unwrap();
        let bits_per_block = bits_per_block(block_states.palette.len());
        let entries_per_long = 64 / bits_per_block;
        let mask = (1u64 << bits_per_block) - 1;

        // Minecraft's section block index is y * 256 + z * 16 + x.
        let index = local_y * 256 + z * 16 + x;
        let palette_index = read_packed_index(
            data,
            index,
            bits_per_block,
            entries_per_long,
            mask,
        );
        block_states.palette.get(palette_index)?.name.clone()
    };

    Some(name)
}

/// Find the first non-air, non-water block strictly below the water surface at
/// column `(x, z)`, whose surface (top block) sits at absolute height
/// `top_y`.
///
/// Sections are scanned from highest to lowest. For each section only the Y
/// values that lie strictly below `top_y` are considered, scanned from top to
/// bottom. The first solid block found is returned.
fn find_block_under_water(
    chunk: &Chunk,
    x: usize,
    z: usize,
    top_y: i32,
) -> Option<String> {
    let mut sections: Vec<&Section> = chunk.sections.iter().collect();
    sections.sort_unstable_by(|a, b| b.y.cmp(&a.y));

    for section in &sections {
        let section_base = section.y as i32 * 16;

        // Highest local Y that is strictly below the water surface.
        let max_local = (top_y - 1 - section_base) as i32;
        if max_local < 0 {
            // This whole section sits at or above the surface.
            continue;
        }
        let max_local = (max_local as usize).min(15);

        for local_y in (0..=max_local).rev() {
            if let Some(name) = read_block_at(section, local_y, x, z) {
                if !is_air(&name) && !is_water(&name) {
                    return Some(name);
                }
            }
        }
    }

    None
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

/// Whether a block name is a (flowing or still) water block.
pub(crate) fn is_water(name: &str) -> bool {
    matches!(name, "minecraft:water" | "minecraft:flowing_water")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_water_lookup_finds_the_first_solid_block_below() {
        // A whole-section single-palette (no data) section: section at y=4 is
        // all water (surface), section at y=3 is all dirt (the floor).
        let section = |y: i8, name: &str| Section {
            y,
            block_states: Some(BlockStates {
                palette: vec![PaletteEntry { name: name.to_string() }],
                data: None,
            }),
        };
        let chunk = Chunk {
            sections: vec![section(4, "minecraft:water"), section(3, "minecraft:dirt")],
        };

        let top = get_top_blocks(&chunk);

        // Every column has a water surface sitting at the top of section y=4.
        assert_eq!(top.blocks[0].as_deref(), Some("minecraft:water"));
        assert_eq!(top.heights[0], Some(4 * 16 + 15));
        // And the first solid block beneath it is the dirt floor.
        assert_eq!(top.under[0].as_deref(), Some("minecraft:dirt"));
    }
}

