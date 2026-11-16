# Render comparison prototype

A separate, local visual study for Motion Choreographer. It does **not** replace or change the app's production renderer. There is no API, hardware control, telemetry, publishing, or build integration.

Run from the repository root:

```sh
python3 -m http.server 8765 --bind 127.0.0.1 --directory design/render-prototype
```

Open <http://localhost:8765>. Use an HTTP server; browsers restrict module and JSON loading from `file://` URLs. All runtime dependencies and assets are included, so the demo makes no external requests.

## Compare one choice at a time

- **Current blocks + Simple shading**: original mesh and an approximation of the existing CPU lighting equation, drawn in WebGL.
- **Rounded models + Simple shading**: isolates geometry changes, including human silhouettes and the PTZ head, lens, yoke, and inverted support.
- **Current blocks + Materials + shadows**: isolates the proposed lighting/material treatment on the original mesh.
- **Rounded models + Materials + shadows**: combines the two proposals.

Choose Wide, Lectern, Overview, or Camera model. Each preset starts with identical camera position, horizontal field of view, aspect ratio, and target on both sides. Dragging orbits only the candidate; **Reset view** restores a matched comparison. The reference stays a still and is explicitly labelled as such.

## What the reference contains

The left-hand PNGs are actual 960 × 540 output of `ptz_scene::render::draw_scene` using the existing Rust renderer's directional/hemisphere lighting and fog. They are not WebGL screenshots or artist impressions. The stage geometry was copied directly from `builtin()` and `person()` in `crates/app/src/stage.rs`, based on repository revision `33a3413`. `stage-baseline.json` contains the exact positions, triangles and face colours used for those captures.

The Camera model reference uses a small test floor and two cameras assembled from the existing block marker dimensions, one upright and one inverted. The same fixture is exported as `rig-baseline.json`; the inverted support demonstrates mounting orientation without relying on a production renderer change. Reference captures exclude app overlays, grids, guides, and labels.

World axes are metres, Z up, +Y forward. Preset definitions:

| View | Eye (m) | Target (m) | Horizontal FOV |
|---|---|---|---|
| Wide | (0, 0, 1.2) | (0, 12, 1.2) | 60° |
| Lectern | (0, 0, 1.2) | (5.3, 12.45, 1.8) | 24° |
| Overview | (15, −11, 11) | (0, 8, 1.8) | 60° |
| Camera model | (3, 3, 2.7) | (0, 0, 1.65) | 50° |

The detailed candidate intentionally changes the model: rounded people, neutral stage materials, front fascia, stairs, screen frame, and a recognisable PTZ housing. The rich shading mode uses standard roughness/metalness materials, hemisphere and directional lighting, PCF soft shadows, ACES tone mapping, and antialiasing. No texture maps, imported models, or photorealistic assets are included.

## Scope and limits

This establishes a visual direction. It does not establish a native-renderer migration plan or promise a performance improvement. Three.js is a convenient browser prototyping renderer; its use here is not a recommendation to embed a browser in the native app.

The live statistic is the median browser animation-frame interval over up to 90 frames. It includes browser scheduling and this machine's rendering behavior. It is **not** GPU execution time, a CPU-versus-GPU benchmark, or a result for twelve views, imported venues, native egui integration, or production hardware. Headless validation used Chromium with software WebGL, so its timings should not guide backend selection. Shadows are rebuilt only when scene/light settings change.

An implementation decision should follow a separate native test with representative imported models, multiple simultaneous camera previews, GPU compatibility/fallback handling, and measured latency. Better geometry could also be added to the existing renderer independently.

## Verification and dependencies

`smoke-test.cjs` uses Playwright and an installed Chromium browser to check all presets and toggle combinations, orbit/reset, reference image loading, browser errors, and the mobile layout. It saves comparison screenshots. Start the HTTP server above, then run with Playwright available to Node:

```sh
node design/render-prototype/smoke-test.cjs
```

Set `CHROMIUM_PATH` if Chromium is not at `/usr/bin/chromium`, and `PROTOTYPE_URL` to change the default URL. The test explicitly uses software WebGL for portability and does not measure production performance.

Three.js **0.170.0 / r170** is pinned and vendored in `vendor/three.module.min.js` from the npm distribution at <https://cdn.jsdelivr.net/npm/three@0.170.0/build/three.module.min.js>. Its MIT license is included in `vendor/THREE-LICENSE.txt`. Generated meshes, baseline exports and comparison UI are local project artifacts.
