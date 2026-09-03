//! Command-line argument parsing, usage text, and the shared sampling
//! constants used by both the CLI layer and the rendering layer.

use std::env;

/// Supersampling factor for `--supersample`: each block is drawn as a solid
/// `SUPER_SAMPLE x SUPER_SAMPLE` pixel square (5 pixels per block) with no
/// interpolation between blocks.
pub(crate) const SUPER_SAMPLE: u32 = 5;

/// Hypersampling factor for `--hypersampling`: each block is drawn as a solid
/// `HYPER_SAMPLE x HYPER_SAMPLE` pixel square (15 pixels per block) with no
/// interpolation between blocks. Like `SUPER_SAMPLE` but zoomed in 3x.
pub(crate) const HYPER_SAMPLE: u32 = 15;

/// Parsed command-line arguments.
pub(crate) struct Args {
    pub(crate) world_path: String,
    pub(crate) single: bool,
    pub(crate) scale: u32,
    pub(crate) dim: i32,
    pub(crate) shadows: bool,
    pub(crate) supersample: bool,
    pub(crate) hypersample: bool,
    pub(crate) ambient_occlusion: bool,
    pub(crate) bloom: bool,
    pub(crate) transparency: bool,
    pub(crate) night: bool,
}

impl Args {
    /// Pixels-per-block upsample factor: `Some(15)` for `--hypersampling`,
    /// `Some(5)` for `--supersample`, or `None` for native / `--scale` mode
    /// (in which case [`Args::scale`] applies instead).
    ///
    /// Only exercised by the unit tests below, so allow the dead-code warning
    /// in non-test (binary) builds.
    #[allow(dead_code)]
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


pub(crate) fn print_usage() {
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
                             combined with -hs/--hypersampling or -z/--scale\n\
         -hs, --hypersampling Render each block as a solid 15x15 pixel square\n\
                             (15 pixels per block, no interpolation) -- like\n\
                             -ss but zoomed in 3x. When shadows are enabled\n\
                             they are resolved at pixel resolution so their\n\
                             edges stay smooth. Cannot be combined with\n\
                             -ss/--supersample or -z/--scale\n\
         -ao, --ambient-occlusion Draw a soft black gradient on the edge of a\n\
                             block whose neighbour stands exactly one block\n\
                             higher. The gradient covers the outer 1/4 of the\n\
                             block, is black at the shared edge and fades to\n\
                             almost invisible toward the block's centre. Only\n\
                             available together with -ss/--supersample or\n\
                             -hs/--hypersampling\n\
         -b, --bloom          Make light-emitting blocks (torches, lava,\n\
                              glowstone, lanterns, ...) cast a radial gradient\n\
                              of their light color onto the surrounding blocks.\n\
                              Only available together with -ss/--supersample or\n\
                              -hs/--hypersampling\n\
         -t, --transparency   Render water semi-transparently: a water surface\n\
                             is blended with the first non-water block beneath\n\
                             it, so the block under the water is faintly\n\
                             visible\n\
         -n, --night          Night mode: the world becomes very dark, shadows\n\
                             become almost pitch black, and bloom (if enabled)\n\
                             gets larger with more falloff distance so lights\n\
                             illuminate a wider area\n\
         -h, --help           Show this message"
    );
}


pub(crate) fn parse_args() -> Result<Args, String> {
    let mut world_path: Option<String> = None;
    let mut single = false;
    let mut scale: u32 = 1;
    let mut scale_set = false;
    let mut dim: i32 = 0;
    let mut shadows = false;
    let mut supersample = false;
    let mut hypersample = false;
    let mut ambient_occlusion = false;
    let mut bloom = false;
    let mut transparency = false;
    let mut night = false;

    let raw: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;

    while i < raw.len() {
        let arg = raw[i].as_str();

        match arg {
            "--single" | "--big" | "-s" => single = true,
            "--shadows" | "--shadow" | "-r" => shadows = true,
            "--supersample" | "-ss" => supersample = true,
            "--hypersampling" | "--hypersample" | "-hs" => hypersample = true,
            "--ambient-occlusion" | "-ao" => ambient_occlusion = true,
            "--bloom" | "-b" => bloom = true,
            "--transparency" | "-t" => transparency = true,
            "--night" | "-n" => night = true,
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

    if supersample && hypersample {
        return Err(
            "--supersample (-ss) and --hypersampling (-hs) cannot be combined".into(),
        );
    }
    if hypersample && scale_set {
        return Err(
            "--hypersampling (-hs) cannot be combined with --scale (-z/--scale)".into(),
        );
    }
    if supersample && scale_set {
        return Err(
            "--supersample (-ss) cannot be combined with --scale (-z/--scale)".into(),
        );
    }
    if ambient_occlusion && !(supersample || hypersample) {
        return Err(
            "--ambient-occlusion (-ao) requires --supersample (-ss) or --hypersampling (-hs)"
                .into(),
        );
    }
    if bloom && !(supersample || hypersample) {
        return Err(
            "--bloom (-b) requires --supersample (-ss) or --hypersampling (-hs)".into(),
        );
    }

    let world_path =
        world_path.ok_or_else(|| "Missing <path-to-world> argument".to_string())?;

    Ok(Args {
        world_path,
        single,
        scale,
        dim,
        shadows,
        supersample,
        hypersample,
        ambient_occlusion,
        bloom,
        transparency,
        night,
    })
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
/// [`crate::dimension::dimension_info`], which also reports the set of valid
/// ids.
pub(crate) fn parse_dim(value: &str) -> Result<i32, String> {
    value
        .parse::<i32>()
        .map_err(|_| format!("Invalid dimension id: {value:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dim_validates_input() {
        assert_eq!(parse_dim("0"), Ok(0));
        assert_eq!(parse_dim("1"), Ok(1));
        assert_eq!(parse_dim("-1"), Ok(-1));
        assert!(parse_dim("abc").is_err());
        assert!(parse_dim("1.5").is_err());
    }

    #[test]
    fn upsample_factor_selects_the_requested_mode() {
        let args = |supersample: bool, hypersample: bool| Args {
            world_path: String::new(),
            single: false,
            scale: 1,
            dim: 0,
            shadows: false,
            supersample,
            hypersample,
            ambient_occlusion: false,
            bloom: false,
            transparency: false,
            night: false,
        };
        assert_eq!(args(false, false).upsample_factor(), None);
        assert_eq!(args(true, false).upsample_factor(), Some(SUPER_SAMPLE));
        assert_eq!(args(false, true).upsample_factor(), Some(HYPER_SAMPLE));
        // Hypersample wins if both were somehow set (parse_args forbids it).
        assert_eq!(args(true, true).upsample_factor(), Some(HYPER_SAMPLE));
    }
}



