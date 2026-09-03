# MCWorldraw

A tool to render top-down view of a Minecraft world

Supports modern versions of Java edition, legacy untested.

## Usage

Example command:
```
cargo run --release -- "path/to/world" -s -r -hs -ao -t -b
```

## Launch flags:

-s = Render the whole world as a single image
-z int = Downscale the map (2 will change the scale to 2 blocks per pixel, etc...)
-d -1,0,1 = Dimension to render, custom dimensions are not supported. 0 is overworld, 1 is the nether and -1 is the end
-r = enables shadows, unsupported without -s
-ss = SuperSample: Bumps the resolution, now each block is 5 pixels in size. can't use with -z
-hs = HyperSample: Bumps the resolution more, now each block is 15 pixels in size. can't use with -z
-ao = ambient occlusion on block edges. Only with Super/Hypersampling
-b = Bloom on lava, torches and glowstone. Super/Hypersampling only
-t = Transparency on water, allowing to see underwater blocks
-n = Night mode (WIP)
