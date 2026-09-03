//! Rendering color palette for all Minecraft blocks.
//!
//! Each block is mapped to a top-down view color `[R, G, B]`. Colors are
//! hand-picked to approximate the appearance of the block's top face in
//! typical Minecraft lighting. Unknown blocks fall back to a stable
//! hash-derived color so they remain visually distinct.

/// Map a Minecraft block name to a top-down view color.
///
/// Well-known blocks get hand-picked colors; anything else falls back to a
/// stable color derived from a hash of the name so distinct unknown blocks
/// are still visually distinguishable.
pub fn block_color(name: &str) -> [u8; 3] {
    match name {
        // ─────────────────────────────────────────────
        // Ground & Terrain
        // ─────────────────────────────────────────────
        "minecraft:grass_block" => [106, 170, 64],
        "minecraft:grass_block_fern" => [106, 170, 64],
        "minecraft:dirt" => [134, 96, 67],
        "minecraft:dirt_with_roots" => [124, 90, 60],
        "minecraft:coarse_dirt" => [134, 96, 67],
        "minecraft:rooted_dirt" => [124, 90, 60],
        "minecraft:podzol" => [95, 70, 42],
        "minecraft:podzol_snow" => [240, 245, 250],
        "minecraft:mycelium" => [120, 104, 120],
        "minecraft:moss_block" => [80, 130, 60],
        "minecraft:moss_carpet" => [80, 130, 60],
        "minecraft:mud" => [78, 60, 46],
        "minecraft:mud_bricks" => [115, 85, 62],
        "minecraft:packed_mud" => [90, 70, 55],

        // ─────────────────────────────────────────────
        // Stone family
        // ─────────────────────────────────────────────
        "minecraft:stone" => [125, 125, 125],
        "minecraft:cobblestone" => [118, 118, 118],
        "minecraft:mossy_cobblestone" => [100, 110, 95],
        "minecraft:stone_bricks" => [125, 125, 125],
        "minecraft:mossy_stone_bricks" => [105, 115, 100],
        "minecraft:cracked_stone_bricks" => [120, 120, 120],
        "minecraft:chiseled_stone_bricks" => [130, 130, 130],
        "minecraft:smooth_stone" => [140, 140, 140],
        "minecraft:granite" => [138, 73, 56],
        "minecraft:polished_granite" => [150, 80, 60],
        "minecraft:smooth_granite" => [155, 85, 65],
        "minecraft:diorite" => [200, 200, 200],
        "minecraft:polished_diorite" => [210, 210, 210],
        "minecraft:smooth_diorite" => [215, 215, 215],
        "minecraft:andesite" => [136, 136, 136],
        "minecraft:polished_andesite" => [145, 145, 145],
        "minecraft:smooth_andesite" => [150, 150, 150],
        "minecraft:deepslate" => [67, 67, 67],
        "minecraft:cobbled_deepslate" => [72, 72, 72],
        "minecraft:polished_deepslate" => [80, 80, 80],
        "minecraft:deepslate_bricks" => [70, 70, 70],
        "minecraft:deepslate_tiles" => [65, 65, 65],
        "minecraft:mossy_deepslate" => [55, 65, 55],
        "minecraft:cracked_deepslate_bricks" => [65, 65, 65],
        "minecraft:cracked_deepslate_tiles" => [60, 60, 60],
        "minecraft:chiseled_deepslate" => [75, 75, 75],
        "minecraft:reinforced_deepslate" => [70, 70, 70],
        "minecraft:tuff" => [95, 95, 95],
        "minecraft:tuff_bricks" => [90, 90, 90],
        "minecraft:calcite" => [233, 231, 226],
        "minecraft:dripstone_block" => [170, 145, 120],
        "minecraft:pointed_dripstone" => [165, 140, 115],
        "minecraft:basalt" => [110, 108, 108],
        "minecraft:polished_basalt" => [115, 113, 113],
        "minecraft:smooth_basalt" => [120, 118, 118],
        "minecraft:bedrock" => [110, 110, 110],
        "minecraft:obsidian" => [28, 24, 38],
        "minecraft:crying_obsidian" => [30, 30, 60],
        "minecraft:bricks" => [150, 96, 84],

        // ─────────────────────────────────────────────
        // Sand & Gravel
        // ─────────────────────────────────────────────
        "minecraft:sand" => [219, 209, 160],
        "minecraft:red_sand" => [190, 110, 70],
        "minecraft:sandstone" => [217, 209, 158],
        "minecraft:smooth_sandstone" => [220, 212, 165],
        "minecraft:chiseled_sandstone" => [215, 207, 155],
        "minecraft:cut_sandstone" => [218, 210, 160],
        "minecraft:red_sandstone" => [189, 110, 60],
        "minecraft:smooth_red_sandstone" => [192, 115, 65],
        "minecraft:chiseled_red_sandstone" => [187, 108, 58],
        "minecraft:cut_red_sandstone" => [190, 112, 62],
        "minecraft:gravel" => [125, 125, 125],

        // ─────────────────────────────────────────────
        // Snow & Ice
        // ─────────────────────────────────────────────
        "minecraft:snow" => [247, 250, 253],
        "minecraft:snow_block" => [245, 248, 252],
        "minecraft:powder_snow" => [240, 245, 250],
        "minecraft:ice" => [126, 175, 232],
        "minecraft:packed_ice" => [145, 190, 231],
        "minecraft:blue_ice" => [98, 162, 232],
        "minecraft:frosted_ice" => [130, 180, 235],

        // ─────────────────────────────────────────────
        // Water & Lava
        // ─────────────────────────────────────────────
        "minecraft:water" => [62, 121, 201],
        "minecraft:flowing_water" => [62, 121, 201],
        "minecraft:lava" => [243, 118, 53],
        "minecraft:flowing_lava" => [243, 118, 53],

        // ─────────────────────────────────────────────
        // Nether
        // ─────────────────────────────────────────────
        "minecraft:netherrack" => [135, 58, 52],
        "minecraft:nether_bricks" => [45, 30, 30],
        "minecraft:red_nether_bricks" => [100, 40, 35],
        "minecraft:cracked_nether_bricks" => [40, 28, 28],
        "minecraft:chiseled_nether_bricks" => [50, 35, 35],
        "minecraft:quartz_block" => [234, 228, 221],
        "minecraft:smooth_quartz" => [240, 235, 228],
        "minecraft:quartz_bricks" => [230, 224, 217],
        "minecraft:chiseled_quartz_block" => [235, 230, 223],
        "minecraft:quartz_pillar" => [232, 226, 219],
        "minecraft:soul_sand" => [120, 110, 105],
        "minecraft:soul_soil" => [110, 100, 95],
        "minecraft:magma_block" => [190, 60, 30],
        "minecraft:nether_wart_block" => [96, 32, 110],
        "minecraft:warped_wart_block" => [120, 210, 195],
        "minecraft:blackstone" => [42, 42, 42],
        "minecraft:polished_blackstone" => [50, 50, 50],
        "minecraft:chiseled_blackstone" => [48, 48, 48],
        "minecraft:polished_blackstone_bricks" => [46, 46, 46],
        "minecraft:gilded_blackstone" => [52, 52, 55],
        "minecraft:ancient_debris" => [100, 75, 75],
        "minecraft:crimson_nylium" => [160, 80, 120],
        "minecraft:warped_nylium" => [100, 180, 170],
        "minecraft:crimson_roots" => [140, 70, 100],
        "minecraft:warped_roots" => [100, 170, 160],
        "minecraft:shroomlight" => [220, 130, 100],
        "minecraft:weeping_vines" => [160, 60, 100],
        "minecraft:twisting_vines" => [100, 180, 170],
        "minecraft:nether_sprouts" => [140, 80, 110],
        "minecraft:nether_wart" => [200, 40, 40],
        "minecraft:chain" => [80, 80, 80],
        "minecraft:respawn_anchor" => [20, 20, 20],
        "minecraft:lodestone" => [60, 60, 60],
        "minecraft:resin_block" => [200, 150, 100],
        "minecraft:resin_clumps" => [200, 150, 100],

        // ─────────────────────────────────────────────
        // The End
        // ─────────────────────────────────────────────
        "minecraft:end_stone" => [221, 219, 165],
        "minecraft:end_stone_bricks" => [210, 208, 155],
        "minecraft:purpur_block" => [197, 168, 220],
        "minecraft:purpur_pillar" => [200, 175, 225],
        "minecraft:purpur_stairs" => [195, 165, 218],
        "minecraft:chorus_plant" => [180, 160, 210],
        "minecraft:chorus_flower" => [200, 180, 230],
        "minecraft:end_rod" => [220, 230, 200],
        "minecraft:ender_chest" => [30, 30, 50],
        "minecraft:dragon_egg" => [40, 60, 40],

        // ─────────────────────────────────────────────
        // Wood: Planks
        // ─────────────────────────────────────────────
        "minecraft:oak_planks" => [162, 130, 78],
        "minecraft:spruce_planks" => [112, 84, 50],
        "minecraft:birch_planks" => [192, 175, 121],
        "minecraft:jungle_planks" => [160, 134, 68],
        "minecraft:acacia_planks" => [168, 121, 53],
        "minecraft:dark_oak_planks" => [66, 43, 20],
        "minecraft:mangrove_planks" => [145, 104, 86],
        "minecraft:cherry_planks" => [185, 143, 133],
        "minecraft:pale_oak_planks" => [190, 178, 124],
        "minecraft:crimson_planks" => [148, 60, 68],
        "minecraft:warped_planks" => [50, 150, 150],
        "minecraft:bamboo_planks" => [200, 190, 100],
        "minecraft:bamboo_mosaic" => [200, 195, 105],

        // ─────────────────────────────────────────────
        // Wood: Logs & Stems
        // ─────────────────────────────────────────────
        "minecraft:oak_log" => [104, 82, 50],
        "minecraft:spruce_log" => [55, 40, 25],
        "minecraft:birch_log" => [226, 224, 216],
        "minecraft:jungle_log" => [134, 114, 62],
        "minecraft:acacia_log" => [148, 103, 62],
        "minecraft:dark_oak_log" => [46, 35, 16],
        "minecraft:mangrove_log" => [120, 90, 70],
        "minecraft:cherry_log" => [150, 105, 95],
        "minecraft:pale_oak_log" => [180, 165, 120],
        "minecraft:crimson_stem" => [130, 50, 60],
        "minecraft:warped_stem" => [40, 140, 140],
        "minecraft:bamboo" => [180, 170, 90],
        "minecraft:bamboo_block" => [180, 170, 90],
        "minecraft:chiseled_bamboo_block" => [175, 165, 85],

        // ─────────────────────────────────────────────
        // Wood: Inner Bark
        // ─────────────────────────────────────────────
        "minecraft:oak_wood" => [95, 75, 45],
        "minecraft:spruce_wood" => [50, 38, 22],
        "minecraft:birch_wood" => [220, 218, 210],
        "minecraft:jungle_wood" => [130, 110, 58],
        "minecraft:acacia_wood" => [144, 100, 58],
        "minecraft:dark_oak_wood" => [42, 33, 15],
        "minecraft:mangrove_wood" => [115, 85, 65],
        "minecraft:cherry_wood" => [145, 100, 90],
        "minecraft:pale_oak_wood" => [175, 160, 115],
        "minecraft:crimson_hyphae" => [125, 48, 58],
        "minecraft:warped_hyphae" => [38, 135, 135],

        // ─────────────────────────────────────────────
        // Stripped Logs
        // ─────────────────────────────────────────────
        "minecraft:stripped_oak_log" => [130, 105, 60],
        "minecraft:stripped_spruce_log" => [75, 55, 35],
        "minecraft:stripped_birch_log" => [235, 233, 225],
        "minecraft:stripped_jungle_log" => [150, 130, 70],
        "minecraft:stripped_acacia_log" => [160, 115, 70],
        "minecraft:stripped_dark_oak_log" => [65, 48, 25],
        "minecraft:stripped_mangrove_log" => [140, 105, 82],
        "minecraft:stripped_cherry_log" => [165, 118, 108],
        "minecraft:stripped_pale_oak_log" => [195, 182, 135],
        "minecraft:stripped_crimson_stem" => [140, 60, 70],
        "minecraft:stripped_warped_stem" => [50, 155, 155],

        // ─────────────────────────────────────────────
        // Stripped Wood
        // ─────────────────────────────────────────────
        "minecraft:stripped_oak_wood" => [125, 100, 55],
        "minecraft:stripped_spruce_wood" => [70, 52, 32],
        "minecraft:stripped_birch_wood" => [232, 230, 222],
        "minecraft:stripped_jungle_wood" => [145, 125, 65],
        "minecraft:stripped_acacia_wood" => [155, 110, 65],
        "minecraft:stripped_dark_oak_wood" => [60, 45, 22],
        "minecraft:stripped_mangrove_wood" => [135, 100, 78],
        "minecraft:stripped_cherry_wood" => [160, 115, 102],
        "minecraft:stripped_pale_oak_wood" => [190, 178, 130],
        "minecraft:stripped_crimson_hyphae" => [135, 55, 65],
        "minecraft:stripped_warped_hyphae" => [45, 150, 150],

        // ─────────────────────────────────────────────
        // Leaves
        // ─────────────────────────────────────────────
        "minecraft:oak_leaves" => [55, 120, 35],
        "minecraft:spruce_leaves" => [40, 90, 30],
        "minecraft:birch_leaves" => [70, 140, 50],
        "minecraft:jungle_leaves" => [45, 105, 25],
        "minecraft:acacia_leaves" => [80, 145, 50],
        "minecraft:dark_oak_leaves" => [50, 110, 30],
        "minecraft:mangrove_leaves" => [50, 115, 40],
        "minecraft:cherry_leaves" => [220, 150, 170],
        "minecraft:pale_oak_leaves" => [100, 160, 80],
        "minecraft:azalea_leaves" => [60, 130, 45],
        "minecraft:flowering_azalea_leaves" => [220, 140, 165],
        "minecraft:spore_blossom" => [180, 100, 80],

        // ─────────────────────────────────────────────
        // Plants & Flowers
        // ─────────────────────────────────────────────
        "minecraft:short_grass" => [80, 150, 50],
        "minecraft:tall_grass" => [80, 150, 50],
        "minecraft:fern" => [75, 145, 45],
        "minecraft:large_fern" => [75, 145, 45],
        "minecraft:dandelion" => [255, 220, 50],
        "minecraft:poppy" => [220, 50, 50],
        "minecraft:blue_orchid" => [100, 80, 220],
        "minecraft:allium" => [180, 130, 220],
        "minecraft:azure_bluet" => [80, 100, 200],
        "minecraft:red_tulip" => [220, 50, 50],
        "minecraft:orange_tulip" => [240, 130, 40],
        "minecraft:white_tulip" => [240, 240, 240],
        "minecraft:pink_tulip" => [240, 150, 200],
        "minecraft:oxeye_daisy" => [250, 250, 250],
        "minecraft:cornflower" => [80, 100, 220],
        "minecraft:lily_of_the_valley" => [240, 240, 200],
        "minecraft:wither_rose" => [80, 40, 60],
        "minecraft:sunflower" => [250, 200, 50],
        "minecraft:lily_pad" => [60, 140, 50],
        "minecraft:seagrass" => [50, 160, 80],
        "minecraft:kelp" => [50, 150, 70],
        "minecraft:sugar_cane" => [140, 180, 80],
        "minecraft:cactus" => [60, 130, 50],
        "minecraft:sweet_berries" => [180, 50, 60],
        "minecraft:glow_berries" => [220, 100, 130],
        "minecraft:glow_lichen" => [100, 220, 150],
        "minecraft:hanging_roots" => [100, 70, 50],
        "minecraft:big_dripleaf" => [60, 140, 50],
        "minecraft:small_dripleaf" => [60, 140, 50],
        "minecraft:azalea" => [60, 130, 45],
        "minecraft:flowering_azalea" => [220, 140, 165],
        "minecraft:vines" => [50, 120, 40],
        "minecraft:brown_mushroom" => [180, 150, 100],
        "minecraft:red_mushroom" => [200, 50, 50],
        "minecraft:mushroom_stem" => [220, 210, 180],
        "minecraft:crimson_fungus" => [180, 70, 80],
        "minecraft:warped_fungus" => [80, 180, 170],
        "minecraft:red_mushroom_block" => [200, 60, 50],
        "minecraft:brown_mushroom_block" => [180, 150, 100],
        "minecraft:pale_mushroom_block" => [200, 200, 170],
        "minecraft:turtle_egg" => [180, 160, 140],

        // ─────────────────────────────────────────────
        // Crops & Food Blocks
        // ─────────────────────────────────────────────
        "minecraft:wheat" => [180, 160, 80],
        "minecraft:carrots" => [220, 120, 40],
        "minecraft:potatoes" => [180, 160, 100],
        "minecraft:beetroot" => [180, 60, 80],
        "minecraft:melon" => [100, 180, 50],
        "minecraft:melon_stem" => [80, 140, 50],
        "minecraft:pumpkin" => [220, 140, 40],
        "minecraft:carved_pumpkin" => [220, 140, 40],
        "minecraft:lit_pumpkin" => [240, 180, 60],
        "minecraft:pumpkin_stem" => [80, 140, 50],
        "minecraft:honey_block" => [220, 180, 60],
        "minecraft:honeycomb_block" => [230, 190, 70],
        "minecraft:dried_kelp_block" => [60, 80, 40],
        "minecraft:hay_block" => [190, 170, 70],

        // ─────────────────────────────────────────────
        // Ores
        // ─────────────────────────────────────────────
        "minecraft:coal_ore" => [100, 100, 100],
        "minecraft:deepslate_coal_ore" => [55, 55, 55],
        "minecraft:iron_ore" => [175, 155, 140],
        "minecraft:deepslate_iron_ore" => [100, 90, 80],
        "minecraft:copper_ore" => [160, 110, 80],
        "minecraft:deepslate_copper_ore" => [100, 70, 50],
        "minecraft:gold_ore" => [240, 210, 80],
        "minecraft:deepslate_gold_ore" => [120, 105, 40],
        "minecraft:redstone_ore" => [120, 120, 120],
        "minecraft:deepslate_redstone_ore" => [70, 70, 70],
        "minecraft:lapis_ore" => [80, 80, 180],
        "minecraft:deepslate_lapis_ore" => [50, 50, 100],
        "minecraft:diamond_ore" => [80, 220, 220],
        "minecraft:deepslate_diamond_ore" => [50, 110, 110],
        "minecraft:emerald_ore" => [80, 200, 100],
        "minecraft:deepslate_emerald_ore" => [50, 100, 60],
        "minecraft:nether_gold_ore" => [220, 190, 70],

        // ─────────────────────────────────────────────
        // Metal Blocks
        // ─────────────────────────────────────────────
        "minecraft:coal_block" => [40, 40, 40],
        "minecraft:iron_block" => [220, 220, 220],
        "minecraft:copper_block" => [200, 120, 80],
        "minecraft:gold_block" => [250, 210, 60],
        "minecraft:diamond_block" => [80, 240, 240],
        "minecraft:emerald_block" => [50, 220, 80],
        "minecraft:redstone_block" => [220, 30, 20],
        "minecraft:lapis_block" => [40, 50, 180],
        "minecraft:netherite_block" => [70, 60, 60],
        "minecraft:raw_iron_block" => [180, 160, 140],
        "minecraft:raw_copper_block" => [180, 100, 60],
        "minecraft:raw_gold_block" => [230, 190, 60],
        "minecraft:exposed_copper" => [180, 160, 100],
        "minecraft:weathered_copper" => [120, 180, 140],
        "minecraft:oxidized_copper" => [80, 180, 140],
        "minecraft:chiseled_copper" => [200, 120, 80],
        "minecraft:cut_copper" => [200, 120, 80],
        "minecraft:exposed_chiseled_copper" => [180, 160, 100],
        "minecraft:exposed_cut_copper" => [180, 160, 100],
        "minecraft:weathered_chiseled_copper" => [120, 180, 140],
        "minecraft:weathered_cut_copper" => [120, 180, 140],
        "minecraft:oxidized_chiseled_copper" => [80, 180, 140],
        "minecraft:oxidized_cut_copper" => [80, 180, 140],

        // ─────────────────────────────────────────────
        // Prismarine & Sponge
        // ─────────────────────────────────────────────
        "minecraft:prismarine" => [90, 190, 180],
        "minecraft:prismarine_bricks" => [95, 195, 185],
        "minecraft:dark_prismarine" => [60, 140, 130],
        "minecraft:dark_prismarine_bricks" => [65, 145, 135],
        "minecraft:sea_lantern" => [180, 240, 230],
        "minecraft:sponge" => [220, 220, 100],
        "minecraft:wet_sponge" => [180, 190, 80],

        // ─────────────────────────────────────────────
        // Terracotta
        // ─────────────────────────────────────────────
        "minecraft:terracotta" => [152, 92, 69],
        "minecraft:white_terracotta" => [230, 220, 215],
        "minecraft:orange_terracotta" => [235, 150, 60],
        "minecraft:magenta_terracotta" => [210, 100, 180],
        "minecraft:light_blue_terracotta" => [100, 170, 230],
        "minecraft:yellow_terracotta" => [230, 210, 50],
        "minecraft:lime_terracotta" => [140, 210, 50],
        "minecraft:pink_terracotta" => [235, 170, 190],
        "minecraft:gray_terracotta" => [95, 95, 95],
        "minecraft:light_gray_terracotta" => [170, 170, 170],
        "minecraft:cyan_terracotta" => [90, 210, 210],
        "minecraft:purple_terracotta" => [150, 80, 200],
        "minecraft:blue_terracotta" => [70, 100, 220],
        "minecraft:brown_terracotta" => [110, 75, 50],
        "minecraft:green_terracotta" => [90, 150, 60],
        "minecraft:red_terracotta" => [210, 80, 60],
        "minecraft:black_terracotta" => [45, 45, 45],

        // ─────────────────────────────────────────────
        // Clay
        // ─────────────────────────────────────────────
        "minecraft:clay" => [160, 170, 190],

        // ─────────────────────────────────────────────
        // Wool (all dyes)
        // ─────────────────────────────────────────────
        "minecraft:white_wool" => [240, 240, 240],
        "minecraft:orange_wool" => [240, 150, 50],
        "minecraft:magenta_wool" => [220, 100, 190],
        "minecraft:light_blue_wool" => [110, 180, 240],
        "minecraft:yellow_wool" => [240, 220, 50],
        "minecraft:lime_wool" => [150, 220, 50],
        "minecraft:pink_wool" => [240, 170, 200],
        "minecraft:gray_wool" => [100, 100, 100],
        "minecraft:light_gray_wool" => [180, 180, 180],
        "minecraft:cyan_wool" => [100, 220, 220],
        "minecraft:purple_wool" => [160, 90, 210],
        "minecraft:blue_wool" => [80, 110, 230],
        "minecraft:brown_wool" => [120, 85, 55],
        "minecraft:green_wool" => [95, 155, 65],
        "minecraft:red_wool" => [220, 80, 60],
        "minecraft:black_wool" => [50, 50, 50],

        // ─────────────────────────────────────────────
        // Glass (all dyes)
        // ─────────────────────────────────────────────
        "minecraft:glass" => [190, 224, 235],
        "minecraft:white_stained_glass" => [245, 245, 245],
        "minecraft:orange_stained_glass" => [240, 150, 50],
        "minecraft:magenta_stained_glass" => [220, 100, 190],
        "minecraft:light_blue_stained_glass" => [110, 180, 240],
        "minecraft:yellow_stained_glass" => [240, 220, 50],
        "minecraft:lime_stained_glass" => [150, 220, 50],
        "minecraft:pink_stained_glass" => [240, 170, 200],
        "minecraft:gray_stained_glass" => [100, 100, 100],
        "minecraft:light_gray_stained_glass" => [180, 180, 180],
        "minecraft:cyan_stained_glass" => [100, 220, 220],
        "minecraft:purple_stained_glass" => [160, 90, 210],
        "minecraft:blue_stained_glass" => [80, 110, 230],
        "minecraft:brown_stained_glass" => [120, 85, 55],
        "minecraft:green_stained_glass" => [95, 155, 65],
        "minecraft:red_stained_glass" => [220, 80, 60],
        "minecraft:black_stained_glass" => [50, 50, 50],

        // ─────────────────────────────────────────────
        // Concrete (all dyes)
        // ─────────────────────────────────────────────
        "minecraft:white_concrete" => [240, 240, 240],
        "minecraft:orange_concrete" => [240, 150, 50],
        "minecraft:magenta_concrete" => [220, 100, 190],
        "minecraft:light_blue_concrete" => [110, 180, 240],
        "minecraft:yellow_concrete" => [240, 220, 50],
        "minecraft:lime_concrete" => [150, 220, 50],
        "minecraft:pink_concrete" => [240, 170, 200],
        "minecraft:gray_concrete" => [100, 100, 100],
        "minecraft:light_gray_concrete" => [180, 180, 180],
        "minecraft:cyan_concrete" => [100, 220, 220],
        "minecraft:purple_concrete" => [160, 90, 210],
        "minecraft:blue_concrete" => [80, 110, 230],
        "minecraft:brown_concrete" => [120, 85, 55],
        "minecraft:green_concrete" => [95, 155, 65],
        "minecraft:red_concrete" => [220, 80, 60],
        "minecraft:black_concrete" => [50, 50, 50],

        // ─────────────────────────────────────────────
        // Concrete Powder (all dyes)
        // ─────────────────────────────────────────────
        "minecraft:white_concrete_powder" => [235, 235, 235],
        "minecraft:orange_concrete_powder" => [235, 145, 45],
        "minecraft:magenta_concrete_powder" => [215, 95, 185],
        "minecraft:light_blue_concrete_powder" => [105, 175, 235],
        "minecraft:yellow_concrete_powder" => [235, 215, 45],
        "minecraft:lime_concrete_powder" => [145, 215, 45],
        "minecraft:pink_concrete_powder" => [235, 165, 195],
        "minecraft:gray_concrete_powder" => [95, 95, 95],
        "minecraft:light_gray_concrete_powder" => [175, 175, 175],
        "minecraft:cyan_concrete_powder" => [95, 215, 215],
        "minecraft:purple_concrete_powder" => [155, 85, 205],
        "minecraft:blue_concrete_powder" => [75, 105, 225],
        "minecraft:brown_concrete_powder" => [115, 80, 50],
        "minecraft:green_concrete_powder" => [90, 150, 60],
        "minecraft:red_concrete_powder" => [215, 75, 55],
        "minecraft:black_concrete_powder" => [45, 45, 45],

        // ─────────────────────────────────────────────
        // Lights & Glow
        // ─────────────────────────────────────────────
        "minecraft:glowstone" => [250, 218, 138],
        "minecraft:redstone_lamp" => [200, 80, 50],
        "minecraft:lit_redstone_lamp" => [250, 120, 60],
        "minecraft:beacon" => [150, 220, 240],
        "minecraft:lantern" => [220, 190, 100],
        "minecraft:soul_lantern" => [100, 200, 220],
        "minecraft:torch" => [250, 200, 100],
        "minecraft:soul_torch" => [100, 200, 220],
        "minecraft:campfire" => [220, 120, 40],
        "minecraft:soul_campfire" => [100, 180, 200],
        "minecraft:sea_pickle" => [120, 200, 100],

        // ─────────────────────────────────────────────
        // Decorative & Misc
        // ─────────────────────────────────────────────
        "minecraft:bone_block" => [230, 228, 210],
        "minecraft:slime" => [96, 146, 60],
        "minecraft:bookshelf" => [130, 95, 55],
        "minecraft:scaffolding" => [160, 140, 80],
        "minecraft:target" => [200, 200, 200],
        "minecraft:amethyst_block" => [150, 100, 200],
        "minecraft:budding_amethyst" => [160, 110, 210],
        "minecraft:amethyst_cluster" => [150, 100, 200],
        "minecraft:small_amethyst_bud" => [150, 100, 200],
        "minecraft:medium_amethyst_bud" => [150, 100, 200],
        "minecraft:large_amethyst_bud" => [150, 100, 200],
        "minecraft:sculk" => [10, 20, 30],
        "minecraft:sculk_catalyst" => [15, 25, 35],
        "minecraft:sculk_shrieker" => [15, 25, 35],
        "minecraft:sculk_vein" => [10, 20, 30],
        "minecraft:sculk_sensor" => [15, 25, 35],
        "minecraft:echo_shard_block" => [200, 200, 210],
        "minecraft:redstone_torch" => [220, 50, 30],
        "minecraft:lit_redstone_torch" => [250, 80, 40],
        "minecraft:repeater" => [120, 120, 120],
        "minecraft:comparator" => [120, 120, 120],
        "minecraft:barrier" => [128, 128, 128],
        "minecraft:light_block" => [255, 255, 255],

        // ─────────────────────────────────────────────
        // Fallback: stable hash-based color
        // ─────────────────────────────────────────────
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

    #[test]
    fn known_blocks_get_their_colors() {
        assert_eq!(block_color("minecraft:grass_block"), [106, 170, 64]);
        assert_eq!(block_color("minecraft:water"), [62, 121, 201]);
        assert_eq!(block_color("minecraft:sand"), [219, 209, 160]);
        assert_eq!(block_color("minecraft:netherrack"), [135, 58, 52]);
        assert_eq!(block_color("minecraft:end_stone"), [221, 219, 165]);
        assert_eq!(block_color("minecraft:obsidian"), [28, 24, 38]);
        assert_eq!(block_color("minecraft:glowstone"), [250, 218, 138]);
        assert_eq!(block_color("minecraft:oak_planks"), [162, 130, 78]);
    }

    #[test]
    fn unknown_blocks_are_stable_and_bounded() {
        let a = block_color("minecraft:some_unknown_block_xyz");
        let b = block_color("minecraft:some_unknown_block_xyz");
        assert_eq!(a, b, "hash color must be deterministic");
        for channel in a {
            assert!((30..=210).contains(&channel));
        }
        assert_ne!(
            block_color("minecraft:one_block"),
            block_color("minecraft:two_block"),
            "distinct blocks should get distinct fallback colors"
        );
    }

    #[test]
    fn nether_blocks_have_colors() {
        assert_eq!(block_color("minecraft:soul_sand"), [120, 110, 105]);
        assert_eq!(block_color("minecraft:magma_block"), [190, 60, 30]);
        assert_eq!(block_color("minecraft:crimson_stem"), [130, 50, 60]);
        assert_eq!(block_color("minecraft:warped_stem"), [40, 140, 140]);
        assert_eq!(block_color("minecraft:shroomlight"), [220, 130, 100]);
        assert_eq!(block_color("minecraft:blackstone"), [42, 42, 42]);
    }

    #[test]
    fn end_blocks_have_colors() {
        assert_eq!(block_color("minecraft:purpur_block"), [197, 168, 220]);
        assert_eq!(block_color("minecraft:end_stone_bricks"), [210, 208, 155]);
        assert_eq!(block_color("minecraft:chorus_plant"), [180, 160, 210]);
    }

    #[test]
    fn all_dye_variants_have_distinct_colors() {
        assert_ne!(block_color("minecraft:red_wool"), block_color("minecraft:blue_wool"));
        assert_ne!(block_color("minecraft:white_wool"), block_color("minecraft:black_wool"));
        assert_ne!(block_color("minecraft:red_concrete"), block_color("minecraft:blue_concrete"));
        assert_ne!(block_color("minecraft:red_terracotta"), block_color("minecraft:blue_terracotta"));
        assert_ne!(
            block_color("minecraft:red_stained_glass"),
            block_color("minecraft:blue_stained_glass")
        );
    }

    #[test]
    fn stone_variants_have_colors() {
        assert_eq!(block_color("minecraft:stone"), [125, 125, 125]);
        assert_eq!(block_color("minecraft:cobblestone"), [118, 118, 118]);
        assert_eq!(block_color("minecraft:deepslate"), [67, 67, 67]);
        assert_eq!(block_color("minecraft:calcite"), [233, 231, 226]);
        assert_eq!(block_color("minecraft:tuff"), [95, 95, 95]);
    }

    #[test]
    fn ice_variants_have_colors() {
        assert_eq!(block_color("minecraft:ice"), [126, 175, 232]);
        assert_eq!(block_color("minecraft:packed_ice"), [145, 190, 231]);
        assert_eq!(block_color("minecraft:blue_ice"), [98, 162, 232]);
    }

    #[test]
    fn ore_blocks_have_colors() {
        assert_eq!(block_color("minecraft:diamond_ore"), [80, 220, 220]);
        assert_eq!(block_color("minecraft:deepslate_diamond_ore"), [50, 110, 110]);
        assert_eq!(block_color("minecraft:redstone_ore"), [120, 120, 120]);
        assert_eq!(block_color("minecraft:ancient_debris"), [100, 75, 75]);
    }
}