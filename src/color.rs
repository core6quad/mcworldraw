//! Color math for a map column: the void color, shadow darkening, ambient
//! occlusion darkening, and water transparency blending.
//!
//! [`display_color`] is the single entry point used everywhere a column has to
//! become an RGB color: it resolves a block's base color from the palette and,
//! when `--transparency` is enabled and the surface is water, blends in the
//! block seen through the water.

use crate::chunk::is_water;
use crate::palette::block_color;

/// Color used for columns that have no non-air block (i.e. pure void).
pub(crate) const NO_BLOCK: [u8; 3] = [0, 0, 0];

/// How strongly a shadowed surface is darkened: each RGB channel is multiplied
/// by `SHADOW_NUMERATOR / SHADOW_DENOMINATOR`.
pub(crate) const SHADOW_NUMERATOR: u32 = 55;
pub(crate) const SHADOW_DENOMINATOR: u32 = 100;

/// Transparency blend weights for `--transparency`. When a column's surface is
/// water, its color is blended with the first solid block beneath it so that
/// the block below is only *barely* visible through the water: the water color
/// is multiplied by `WATER_OVER` and the block-beneath color by
/// `WATER_UNDER`. `WATER_OVER >> WATER_UNDER` keeps the block faint.
pub(crate) const WATER_OVER: u32 = 5;
pub(crate) const WATER_UNDER: u32 = 1;

/// Darken a color to its "in shadow" appearance by scaling every channel by
/// [`SHADOW_NUMERATOR`] / [`SHADOW_DENOMINATOR`].
pub(crate) fn shade(rgb: [u8; 3]) -> [u8; 3] {
    [
        ((rgb[0] as u32 * SHADOW_NUMERATOR) / SHADOW_DENOMINATOR) as u8,
        ((rgb[1] as u32 * SHADOW_NUMERATOR) / SHADOW_DENOMINATOR) as u8,
        ((rgb[2] as u32 * SHADOW_NUMERATOR) / SHADOW_DENOMINATOR) as u8,
    ]
}

/// Blend a color toward black by `amount` (0.0 = unchanged, 1.0 = black) for
/// ambient occlusion.
pub(crate) fn apply_ao(rgb: [u8; 3], amount: f32) -> [u8; 3] {
    let keep = 1.0 - amount.clamp(0.0, 1.0);
    [
        (rgb[0] as f32 * keep) as u8,
        (rgb[1] as f32 * keep) as u8,
        (rgb[2] as f32 * keep) as u8,
    ]
}

/// Blend a water color with the color of the block seen through it so the
/// block is only barely visible: water is weighted by [`WATER_OVER`] and the
/// block beneath by [`WATER_UNDER`].
pub(crate) fn blend_water(water: [u8; 3], under: [u8; 3]) -> [u8; 3] {
    let total = WATER_OVER + WATER_UNDER;
    [
        ((water[0] as u32 * WATER_OVER + under[0] as u32 * WATER_UNDER) / total)
            as u8,
        ((water[1] as u32 * WATER_OVER + under[1] as u32 * WATER_UNDER) / total)
            as u8,
        ((water[2] as u32 * WATER_OVER + under[2] as u32 * WATER_UNDER) / total)
            as u8,
    ]
}

/// Display color for a column given its surface block and (for water) the
/// block beneath it.
///
/// Without transparency, or when the surface is not water, this is simply
/// [`block_color`] of the surface block (`NO_BLOCK` for void). With
/// transparency enabled and the surface being water, the water color is blended
/// with the color of the block beneath so the floor is faintly visible; if
/// there is no block beneath, plain water is used.
pub(crate) fn display_color(
    top: &Option<String>,
    under: &Option<String>,
    transparency: bool,
) -> [u8; 3] {
    match top {
        Some(name) => {
            if transparency && is_water(name) {
                if let Some(un) = under {
                    return blend_water(block_color(name), block_color(un));
                }
            }
            block_color(name)
        }
        None => NO_BLOCK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shade_darkens_and_keeps_void_black() {
        assert_eq!(shade([100, 200, 50]), [55, 110, 27]);
        assert_eq!(shade(NO_BLOCK), NO_BLOCK);

        let c = [255u8, 200, 100];
        let s = shade(c);
        assert!(s[0] <= c[0] && s[1] <= c[1] && s[2] <= c[2]);
    }

    #[test]
    fn blend_water_keeps_the_floor_barely_visible() {
        let water = [62u8, 121, 201];
        let floor = [255u8, 255, 255];
        let blended = blend_water(water, floor);

        // It is neither pure water nor pure floor, but a true blend of the two.
        assert_ne!(blended, water);
        assert_ne!(blended, floor);
        for c in 0..3 {
            let lo = water[c].min(floor[c]) as i32;
            let hi = water[c].max(floor[c]) as i32;
            assert!(
                blended[c] as i32 >= lo && blended[c] as i32 <= hi,
                "channel {c} must stay between the two inputs"
            );
        }

        // Water dominates, so the result sits much closer to water than floor.
        let dist = |a: [u8; 3], b: [u8; 3]| {
            a.iter()
                .zip(b.iter())
                .map(|(x, y)| (*x as i32 - *y as i32).abs())
                .sum::<i32>()
        };
        assert!(dist(blended, water) < dist(blended, floor));
    }

    #[test]
    fn display_color_blends_water_with_the_block_beneath() {
        let water = Some("minecraft:water".to_string());
        let dirt = Some("minecraft:dirt".to_string());

        let water_color = block_color("minecraft:water");
        let dirt_color = block_color("minecraft:dirt");

        // Non-water surface is never affected by transparency.
        assert_eq!(
            display_color(&dirt, &None, true),
            dirt_color
        );

        // Water without a block beneath keeps the plain water color.
        assert_eq!(
            display_color(&water, &None, true),
            water_color
        );

        // Water without transparency keeps the plain water color.
        assert_eq!(
            display_color(&water, &dirt, false),
            water_color
        );

        // Water with transparency and a floor below is blended, and stays much
        // closer to the water than to the floor.
        let blended = display_color(&water, &dirt, true);
        assert_ne!(blended, water_color);
        let dist = |a: [u8; 3], b: [u8; 3]| {
            a.iter()
                .zip(b.iter())
                .map(|(x, y)| (*x as i32 - *y as i32).abs())
                .sum::<i32>()
        };
        assert!(dist(blended, water_color) < dist(blended, dirt_color));
    }
}
