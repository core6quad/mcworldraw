//! Elevation lighting: diagonal 45-degree sun shadows and per-pixel ambient
//! occlusion.
//!
//! All three routines operate on a block-resolution height grid (`grid_w x
//! grid_h`, index = `z * grid_w + x`) where [`VOID_H`] marks a void column. In
//! the supersampled modes the height grid is first upsampled to pixel
//! resolution ([`supersampled_heights`]) so shadows and occlusion resolve at
//! pixel resolution and their diagonal edges stay smooth.

use crate::chunk::VOID_H;

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
pub(crate) fn compute_shadows(heights: &[i32], grid_w: usize, grid_h: usize) -> Vec<bool> {
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
/// `--supersample` / `--hypersampling` mode.
///
/// Real columns are multiplied by `s` while void columns keep the [`VOID_H`]
/// sentinel. Multiplying the heights (rather than only the grid size) keeps the
/// 45 degree sun's geometry intact when [`compute_shadows`] runs on the finer
/// grid: a ray that descends one block per block still descends one block per
/// `s` pixels, so shadow lengths are unchanged -- just resolved at pixel
/// resolution, which makes the diagonal edges smooth.
pub(crate) fn supersampled_heights(
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

/// Compute a per-pixel ambient-occlusion darkening field for a supersampled
/// map, in the range 0.0 (no darkening) to 1.0 (fully black).
///
/// `heights` is a row-major `grid_w x grid_h` grid of block-resolution column
/// surface heights (index = `z * grid_w + x`), where [`VOID_H`] marks a void
/// column. For every block, each orthogonal neighbour that stands exactly one
/// block higher casts a soft gradient onto the facing edge of this (lower)
/// block. The gradient covers the outer `s / 4` pixels of the block, is fully
/// black at the shared edge and fades linearly to (almost) transparent toward
/// the block's centre. When several neighbours apply, the strongest darkening
/// at a pixel wins.
pub(crate) fn ambient_occlusion(
    heights: &[i32],
    grid_w: usize,
    grid_h: usize,
    s: usize,
) -> Vec<f32> {
    let pw = grid_w * s;
    let mut out = vec![0.0f32; pw * (grid_h * s)];

    // How many pixels from each edge the gradient reaches: 1/4 of a block.
    let band = s / 4;
    if band == 0 {
        return out;
    }

    for z in 0..grid_h {
        for x in 0..grid_w {
            let h = heights[z * grid_w + x];
            if h == VOID_H {
                continue;
            }

            // Which of the four edges faces a neighbour exactly one block up.
            // Order: top (z-1), bottom (z+1), left (x-1), right (x+1).
            let mut sides = [false; 4];
            if z >= 1 && heights[(z - 1) * grid_w + x] == h + 1 {
                sides[0] = true;
            }
            if z + 1 < grid_h && heights[(z + 1) * grid_w + x] == h + 1 {
                sides[1] = true;
            }
            if x >= 1 && heights[z * grid_w + (x - 1)] == h + 1 {
                sides[2] = true;
            }
            if x + 1 < grid_w && heights[z * grid_w + (x + 1)] == h + 1 {
                sides[3] = true;
            }
            if !sides.iter().any(|b| *b) {
                continue;
            }

            let base_x = x * s;
            let base_z = z * s;

            for dz in 0..s {
                // Distance from each edge, in pixels.
                let d_top = dz;
                let d_bottom = s - 1 - dz;
                for dx in 0..s {
                    let d_left = dx;
                    let d_right = s - 1 - dx;

                    let mut dark = 0.0f32;
                    if sides[0] && d_top < band {
                        dark = dark.max(1.0 - d_top as f32 / band as f32);
                    }
                    if sides[1] && d_bottom < band {
                        dark = dark.max(1.0 - d_bottom as f32 / band as f32);
                    }
                    if sides[2] && d_left < band {
                        dark = dark.max(1.0 - d_left as f32 / band as f32);
                    }
                    if sides[3] && d_right < band {
                        dark = dark.max(1.0 - d_right as f32 / band as f32);
                    }

                    if dark > 0.0 {
                        let pi = (base_z + dz) * pw + (base_x + dx);
                        out[pi] = dark;
                    }
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::HYPER_SAMPLE;

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
    fn hypersample_shadow_is_smooth_at_pixel_resolution() {
        let grid_w = 8;
        let grid_h = 8;
        let s = HYPER_SAMPLE as usize;
        // Flat field at height 0 with a single pillar of height 3 at (x=4, z=1).
        let mut heights = vec![0i32; grid_w * grid_h];
        heights[1 * grid_w + 4] = 3;

        let pixel_heights = supersampled_heights(&heights, grid_w, grid_h, s);
        let pw = grid_w * s;
        let ph = grid_h * s;
        let shadow = compute_shadows(&pixel_heights, pw, ph);

        // A smooth, pixel-resolved shadow means some single block straddles the
        // edge: within its 15x15 pixels there is a mix of lit and shadowed
        // pixels (a block-resolved shadow would make each block uniformly
        // lit or dark).
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
    fn ambient_occlusion_fades_from_edge_to_centre_on_the_lower_block() {
        // 2x1 block grid: left block height 0, right block height 1.
        // Only the left (lower) block receives a gradient, on the edge facing
        // the higher (right) block.
        let grid_w = 2;
        let grid_h = 1;
        let heights = vec![0i32, 1]; // index = z * grid_w + x
        let s = 16usize; // band = s / 4 = 4 pixels
        let pw = grid_w * s;
        let ao = ambient_occlusion(&heights, grid_w, grid_h, s);

        // Left block, right edge (dx = s - 1, the shared edge): fully black.
        let edge = ao[0 * pw + (s - 1)];
        assert!(edge > 0.99, "edge should be near-black, got {edge}");

        // Moving toward the centre the darkening strictly decreases.
        let inner = ao[0 * pw + (s - 1 - 3)]; // 3 px in -> 1 - 3/4 = 0.25
        assert!(inner < edge, "gradient should fade inward");
        assert!((inner - 0.25).abs() < 1e-6, "expected 0.25, got {inner}");

        // Past the 1/4 band (>= band px from the edge) there is no darkening.
        let beyond = ao[0 * pw + (s - 1 - 4)];
        assert_eq!(beyond, 0.0);

        // Well inside the lower block there is no darkening at all.
        assert_eq!(ao[0 * pw + 0], 0.0);

        // The higher (right) block receives no gradient (it is the higher one).
        for dx in 0..s {
            assert_eq!(ao[0 * pw + s + dx], 0.0);
        }
    }

    #[test]
    fn ambient_occlusion_requires_exactly_one_block_of_difference() {
        let grid_w = 2;
        let grid_h = 1;
        let s = 16usize;
        let pw = grid_w * s;

        // Same height on both sides: no AO anywhere.
        let flat = vec![5i32, 5];
        assert!(ambient_occlusion(&flat, grid_w, grid_h, s).iter().all(|v| *v == 0.0));

        // Neighbour two blocks higher (not one): no AO anywhere.
        let steep = vec![5i32, 7];
        assert!(ambient_occlusion(&steep, grid_w, grid_h, s).iter().all(|v| *v == 0.0));

        // Neighbour exactly one block higher: the lower block's facing edge is dark.
        let one = vec![5i32, 6];
        let ao = ambient_occlusion(&one, grid_w, grid_h, s);
        assert!(ao[0 * pw + (s - 1)] > 0.99, "1-block step should produce AO");
    }

    #[test]
    fn ambient_occlusion_ignores_void_columns() {
        // A real block next to a void column: void has no height, so no AO.
        let grid_w = 2;
        let grid_h = 1;
        let s = 16usize;
        let heights = vec![5i32, VOID_H];
        let ao = ambient_occlusion(&heights, grid_w, grid_h, s);
        assert!(ao.iter().all(|v| *v == 0.0));
    }
}
