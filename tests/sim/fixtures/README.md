`atlas.ocat` is a synthetic OCAT v2 fixture, packed with Musette's canonical
`route-cc600` Rust `atlas_pack` example using `version: 2`. Coordinates use
100,000 units/degree. Its name is `Test atlas`; all features are at level zero:

- Country boundary: (1.690, 44.000) to (1.710, 44.030).
- Minor road: (1.690, 44.010) to (1.710, 44.020).
- Path parallel to that road, shifted north by 0.00005° (~5.6 m).
- Department line: (1.695, 44.000) to (1.705, 44.030).
- Closed lake outline: (1.702, 44.018), (1.705, 44.020), (1.707, 44.017).
- Stream: (1.700, 44.000) to (1.700, 44.030).
- Named place `VILLE`: (1.700, 44.015).

No private location or external map data is included.

`atlas-max.ocat.gz` is a compressed synthetic OCAT v1 atlas of 524,018 bytes,
exercising the 512 KiB budget together with a full corridor map in the CC600's
2 MiB PSRAM. It was packed with the same canonical Rust encoder from this JSON
description: name `Memory test`, 965 identical boundary features at level zero
with no label; each has 255 points alternating `(-20000, 60000)` and
`(-19999, 60001)`, starting and ending at the former. Compression uses gzip
with `mtime=0`. Retaining v1 here also checks compatibility with included packs.
