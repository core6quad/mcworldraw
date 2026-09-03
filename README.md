# MCWorldraw

A tool to render top-down view of a Minecraft world

Supports modern versions of Java edition, legacy untested.

## Usage

Example command:
```
cargo run --release -- "path/to/world" -s -r -hs -ao -t -b
```


## Launch flags

| Flag | Description |
|------|-------------|
| `-s` | Render the whole world as a single image |
| `-z <int>` | Downscale the map (`2` = 2 blocks per pixel, etc.) |
| `-d <-1,0,1>` | Dimension to render. `0` = Overworld, `1` = Nether, `-1` = End. Custom dimensions are not supported |
| `-r` | Enables shadows. Requires `-s` |
| `-ss` | SuperSample. Bumps the resolution so each block is 5 pixels in size. Cannot be used with `-z` |
| `-hs` | HyperSample. Bumps the resolution so each block is 15 pixels in size. Cannot be used with `-z` |
| `-ao` | Ambient occlusion on block edges. Requires SuperSampling or HyperSampling |
| `-b` | Bloom on lava, torches, and glowstone. Requires SuperSampling or HyperSampling |
| `-t` | Transparency on water, allowing you to see underwater blocks |
| `-n` | Night mode (WIP) |
