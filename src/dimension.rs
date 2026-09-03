//! Dimension identity and on-disk region folder location.
//!
//! Maps a dimension id to its name, its non-overworld "kind" (if any), and the
//! set of candidate region folders on disk. Disambiguates the legacy
//! `DIM1/region` / `DIM-1/region` folders by inspecting their block content.

use std::fs;
use std::path::{Path, PathBuf};

use fastnbt::from_bytes;
use mca::RegionReader;

use crate::chunk::{Chunk, get_top_blocks};

/// Which non-overworld dimension a chunk's blocks tell us it belongs to.
///
/// Only used to disambiguate the legacy `DIM1` / `DIM-1` folders, whose names
/// do not reliably indicate which dimension they actually hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DimKind {
    Nether,
    End,
}

/// A supported dimension and where its region files live on disk.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DimensionInfo {
    /// Human-readable name, e.g. "overworld".
    pub(crate) name: &'static str,
    /// What non-overworld dimension this is, used to match legacy region
    /// folders by their actual block content. `None` for the overworld.
    pub(crate) kind: Option<DimKind>,
    /// Candidate region folders (relative to the world folder), in order of
    /// preference.
    ///
    /// The modern `dimensions/minecraft:<name>/region` folders are named after
    /// the dimension, so an existing one is unambiguously the right one. The
    /// legacy `DIM1/region` and `DIM-1/region` folders are listed for both
    /// dimensions and disambiguated by content (see [`dimension_region_path`]).
    pub(crate) region_candidates: &'static [&'static str],
    /// Prefix applied to per-chunk output file names so that different
    /// dimensions do not overwrite one another. Empty for the default
    /// overworld, whose output names are left unchanged.
    pub(crate) out_prefix: &'static str,
    /// File name of the single-image output in `--single` mode.
    pub(crate) out_file: &'static str,
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
/// dont wanna deal with custom dimms rn
pub(crate) fn dimension_info(id: i32) -> Result<DimensionInfo, String> {
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
pub(crate) fn dimension_region_path(world_path: &Path, dim: &DimensionInfo) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}


