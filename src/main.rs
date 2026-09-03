//! Renders top-down maps of a Minecraft world from its region files.
//!
//! This binary is a thin composition layer: command-line parsing lives in
//! [`args`], dimension/region-folder handling in [`dimension`], chunk decoding
//! in [`chunk`], color math in [`color`], shadow / supersampling / ambient
//! occlusion in [`light`], pixel rendering in [`render`], and region-file
//! scanning and compositing in [`region`]. Only the program entry point
//! ([`main`]) is defined here.

use std::fs;
use std::path::{Path, PathBuf};

use image::RgbImage;
use indicatif::{ProgressBar, ProgressStyle};

mod palette;
mod args;
mod chunk;
mod color;
mod dimension;
mod light;
mod region;
mod render;

use args::{parse_args, print_usage, HYPER_SAMPLE, SUPER_SAMPLE};
use chunk::{CHUNK_SIZE, VOID_H};
use color::NO_BLOCK;
use dimension::{dimension_info, dimension_region_path};
use light::{ambient_occlusion, bloom, compute_shadows, supersampled_heights};
use region::{
    collect_grid, process_region, process_region_single, process_region_single_ss, scan_region,
    Bounds,
};
use render::{render_shaded_map, render_ss};

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
        // `None` = native / `--scale` mode (downsampling, `scale` applies);
        // `Some(n)` = upsample to `n` pixels per block (5 for -ss, 15 for -hs).
        let upsample = args.upsample_factor();
        let ss = upsample.unwrap_or(1) as i32;
        let blocks_w = (bounds.max_x - bounds.min_x + 1) * (CHUNK_SIZE as i32);
        let blocks_h = (bounds.max_z - bounds.min_z + 1) * (CHUNK_SIZE as i32);

        // In upsample mode each block becomes a solid `ss x ss` pixel square
        // (upsampling), so the image is larger by that factor. Otherwise each
        // output pixel covers `scale x scale` blocks (downsampling), so the
        // image is smaller by that factor (rounded up to a whole pixel).
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

        let res_desc = match upsample {
            Some(SUPER_SAMPLE) => format!("supersampled {} px/block", SUPER_SAMPLE),
            Some(HYPER_SAMPLE) => format!("hypersampled {} px/block", HYPER_SAMPLE),
            Some(n) => format!("{} px/block", n),
            None => format!("scale {}", args.scale),
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

        if args.shadows || args.ambient_occlusion || args.bloom {
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
            // In upsample mode (-ss / -hs) the shadow is resolved at pixel
            // resolution (see `supersampled_heights`) so its diagonal edges
            // stay smooth even though the block colors are drawn as solid
            // squares.
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
                    args.transparency,
                    &pb,
                )?;
            }

            if let Some(factor) = upsample {
                let s = factor as usize;
                let pw = grid_w * s;
                let ph = grid_h * s;
                let pixel_heights = supersampled_heights(&heights, grid_w, grid_h, s);
                let pixel_shadow = if args.shadows {
                    Some(compute_shadows(&pixel_heights, pw, ph))
                } else {
                    None
                };
                drop(pixel_heights);
                let pixel_ao = if args.ambient_occlusion {
                    Some(ambient_occlusion(&heights, grid_w, grid_h, s))
                } else {
                    None
                };
                drop(heights);
                let pixel_bloom = if args.bloom {
                    Some(bloom(&lights, grid_w, grid_h, s))
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
                    args.scale,
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
                    args.transparency,
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
                    args.transparency,
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
        process_region(
            path,
            out_dir,
            args.scale,
            args.upsample_factor().unwrap_or(1),
            dim.out_prefix,
            args.ambient_occlusion,
            args.bloom,
            args.transparency,
            &pb,
        )?;
    }

    pb.finish_with_message("Done");

    println!();
    println!("Generated chunks: {total_chunks}");
    println!("Output directory: {}", out_dir.display());

    Ok(())
}