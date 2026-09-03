//! Renders top-down maps of a Minecraft world from its region files.
//!
//! This binary is a thin composition layer. With arguments it renders from the
//! command line (parsing in [`args`], the shared pipeline in [`pipeline`]);
//! launched with **no** arguments at all it starts the dedicated web server
//! ([`server`]) on port 7878, where a world can be uploaded and rendered
//! through a browser.

use std::process;

use indicatif::{ProgressBar, ProgressStyle};

mod palette;
mod args;
mod chunk;
mod color;
mod dimension;
mod light;
mod region;
mod render;
mod pipeline;
mod server;

use args::{parse_args, print_usage};
use pipeline::{run_render, RenderConfig};

fn main() {
    // Dedicated server mode: the binary was launched with no launch arguments.
    if std::env::args().skip(1).count() == 0 {
        match server::run_server() {
            Ok(()) => return,
            Err(e) => {
                eprintln!("Server error: {e}");
                process::exit(1);
            }
        }
    }

    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            print_usage();
            process::exit(1);
        }
    };

    let config = RenderConfig::from_args(&args);

    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos:>7}/{len:7} {msg}",
        )
        .expect("valid progress bar template"),
    );

    let result = match run_render(&config, &pb) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("{e}");
            print_usage();
            process::exit(1);
        }
    };

    pb.finish_with_message("Done");

    if result.chunks == 0 {
        println!("No chunks found in the world.");
        return;
    }

    match &result.single_image {
        Some(path) => println!(
            "Generated a single {}x{} PNG for {} ({} chunks, {} region files): {}",
            result.width,
            result.height,
            result.dim_name,
            result.chunks,
            result.region_files,
            path.display()
        ),
        None => {
            if let Some(dir) = &result.chunk_dir {
                println!(
                    "Generated {} chunk PNGs for {} ({} region files): {}",
                    result.chunks,
                    result.dim_name,
                    result.region_files,
                    dir.display()
                );
            }
        }
    }
}