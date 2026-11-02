# TM_AO_Combiner

A standalone Rust/egui Windows GUI for TrackMania texture work.
What it does
Drag & drop RGB/Color texture and AO/Prelight texture.
AO is applied as a Multiply layer over RGB.
AO strength 0–100%.
Optional AO inversion.
Optional linear-light multiply.
Preserve RGB alpha (important for textures that use alpha data).
Automatically resize AO/Prelight to RGB dimensions.
Preview the combined result.
Export PNG, TGA, DDS BC3/DXT5, DDS BC7, or uncompressed DDS RGBA8.
Optional automatic DDS mipmaps.
DDS compression quality: Fast / Normal / Slow.
Build on Windows
Install Rust from https://rustup.rs/ and then run:
cargo build --release
The executable will be in:
target\\release\\tm_ao_combiner.exe
For a quick test:
cargo run --release
Blend formula
At 100% AO strength:
output_rgb = rgb * ao
At lower strength the result interpolates between the original RGB and the multiplied RGB.
The AO input is converted to luminance using Rec.709 coefficients. RGB alpha is preserved by default and is never used as the AO multiplier.
