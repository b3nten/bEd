# Embedded HDRI environments

Bed's Model Viewer embeds these unmodified 1K Radiance HDR downloads from
[Poly Haven](https://polyhaven.com/) (formerly HDRI Haven). They are available
offline and serve as both the skybox and the source for diffuse/specular lighting.

Both assets use **CC0 1.0 Universal**. Poly Haven permits redistribution and
commercial use: <https://polyhaven.com/license>. The full dedication is retained
in [CC0-1.0.txt](CC0-1.0.txt).

| Asset | Artist | Source | Bytes | SHA-256 |
| --- | --- | --- | ---: | --- |
| Studio Small 08 | Sergej Majboroda | <https://polyhaven.com/a/studio_small_08> | 1,508,872 | `f6a989f89432eb4eee3191364a9c1ceed195c4ec3544173a3c04fd96cb91d0ba` |
| Kiara 1 Dawn | Greg Zaal | <https://polyhaven.com/a/kiara_1_dawn> | 1,475,077 | `ee70fb8c8fb3e34566802191d83b299e179ecc392b97639e6c750f66e161c8e2` |

Downloaded on 2026-10-08 from the official API's `hdri.1k.hdr` records:

- <https://api.polyhaven.com/files/studio_small_08>
- <https://api.polyhaven.com/files/kiara_1_dawn>

Direct asset URLs:

- <https://dl.polyhaven.org/file/ph-assets/HDRIs/hdr/1k/studio_small_08_1k.hdr>
- <https://dl.polyhaven.org/file/ph-assets/HDRIs/hdr/1k/kiara_1_dawn_1k.hdr>

The API-provided MD5 checksums are respectively
`de3ba64222895aca876b1d1c2e0cf81a` and `fcefd321c51b468ead9d5e1a369a3735`.
Original files are at `crates/bed-plugin-gltf/assets/environments/`. At runtime,
Bed bilinearly resamples them to 256-pixel cube faces in linear RGBA16Float;
this retains HDR values for Bevy's image-based lighting filter.
