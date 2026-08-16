# Battle-sim soldier sprite asset contract

## Contents

- Asset sets and role membership
- Simulator ownership and research evidence
- Coordinate and direction contract
- Image and alpha contract
- Style and ImageGen contract
- Layout and path contract
- Manifest mapping policy
- Identity contract
- Per-action frame-slot semantics
- Quality gates
- Legacy boundary

## Asset sets and role membership

Use lowercase kebab-case ASCII IDs.

Do not define one global role list. For simulator-bound work, each simulator visual profile owns its historically appropriate roles and `set.json` mirrors that membership. A role ID may appear in several profiles/sets with different equipment, or only in the periods and regions where it is appropriate. Examples such as `spearman`, `crossbowman`, `huscarl`, `norman-knight`, or `engineer` are possibilities, not universal requirements.

`set.json` is the source declaration for set membership:

```json
{
  "schemaVersion": 1,
  "id": "europe-14c-photorealistic-full-color",
  "period": {"id": "14c", "label": "14th century CE"},
  "region": {"id": "europe", "label": "Europe"},
  "styleId": "photorealistic-full-color",
  "renderStyle": {
    "preset": "photorealistic-full-color",
    "prompt": "full-color photorealistic historical character cutout; physically plausible materials",
    "avoid": ["monochrome", "illustration", "watermark"],
    "referenceImages": []
  },
  "simulationProfile": "medieval_western",
  "roles": ["men-at-arms", "spearman", "longbowman", "heavy-cavalry"]
}
```

Only create a role directory listed in its set's `roles`. For a simulator-bound set, add or remove roles in `data/visual-profiles/<profile>.toml` and re-import; never edit only the generated set. Keep role IDs semantic and stable; when the historical identity is materially different, prefer a specific ID such as `norman-knight` over overloading a vague global ID.

The simulator profile defines applicable actions. A role may declare only those actions; do not create meaningless filler or an image-only action that simulation states can never select.

Every declared action contains exactly 8 directions x 8 frame slots = 64 images. Frame-slot meaning is action-specific: ordinary cycles use 8 temporal phases, `idle` uses four two-frame styles, and `dead` uses eight static variants.

## Simulator ownership and research evidence

`data/visual-profiles/<profile>.toml` is authoritative for period, region, role ID and description, troop-type index, action membership, playback mode, and the complete mapping from every simulator state to an action. Asset tooling may copy this data into `set.json`, `role.json`, the runtime profile, and manifest, but must never originate or reinterpret it. A historical change to roles or behavior begins in the simulator profile.

The skill owns image-specific historical research. Keep it in each role's `research.json`, not in simulation logic. `research.json` requires at least two independent HTTPS source domains, access dates, and cited claims covering silhouette, garments, armor, weapons, accessories, and palette. Prefer museum, manuscript, archaeological, university, or peer-reviewed sources. Record uncertainty; a category may state that an item is absent or variable when supported, but may not be left unresearched.

Only `apply-research` may transfer the synthesized `visualDefinition` into `role.json.identity` and reviewed role-specific `framePlans` into `role.json`. It must not change `role.json.simulation` or `actionPlayback`. Keep the source list and claim-to-source mapping in `research.json`; store only source IDs and its canonical path in `role.json`. Reject generic biped plans for mounted roles and generic weapon motion that contradicts the researched grip or mechanism.

## Coordinate and direction contract

Keep one orthographic quarter-view camera, projection, elevation, focal scale, horizon-free transparent background, and screen-space lighting direction for the entire set. Rotate the actor around the world vertical axis; never rotate or reframe the camera.

The mapping matches `web/src/render/sprite-atlas.ts`:

| Column | ID | Meaning in the image |
|---:|---|---|
| 0 | `south` | front; actor faces screen-bottom |
| 1 | `southeast` | front-left three-quarter turn |
| 2 | `east` | actor faces screen-left |
| 3 | `northeast` | back-left three-quarter turn |
| 4 | `north` | back; actor faces screen-top |
| 5 | `northwest` | back-right three-quarter turn |
| 6 | `west` | actor faces screen-right |
| 7 | `southwest` | front-right three-quarter turn |

`front.png` is the `south` identity reference. `back.png` is the `north` reference. “Left/right” always means the actor's anatomical left/right.

Use top-left image origin. Fix the ground/foot anchor at `(100, 276)` in every 200x300 frame. For standing bipeds, put the weight-bearing contact at that point; for mounted roles, use the ground contact centered under the mount. For a dead variant, keep its ground-contact baseline at `y=276` and center the resting body's footprint around `x=100` rather than inventing a standing foot. Keep all nontransparent pixels inside `x=4..195`, `y=4..279`; keep the outermost 4-pixel border and rows `280..299` fully transparent. Preserve the anchor rather than centering each silhouette independently.

## Image and alpha contract

- Delivery frame: exactly 200x300 px.
- Working master: generate/edit at 800x1200 or larger with the same 2:3 aspect ratio when possible, then downsample once.
- Color: non-interlaced 8-bit sRGB RGBA PNG with straight alpha.
- Background: real transparency. No white/gray/checkerboard background, scenery, floor plane, border, text, label, or baked shadow.
- Transparent pixels: `A=0` and `RGB=0,0,0`. Partial alpha is only for antialiased edges or genuinely translucent material.
- Framing: full body and role-defining equipment fit without clipping; body height and equipment scale stay fixed.
- Rendering: follow the locked `renderStyle`. The default is full-color photorealism with readable silhouette and physically plausible materials at 200x300. No depth of field, motion blur, grain, bloom, or drifting outlines unless a user-selected style explicitly requires a listed treatment that does not harm animation consistency.

Long weapons and mounts must fit the universal cell. Never silently change cell size. If a required silhouette cannot fit legibly, stop and propose a versioned contract change.

## Style and ImageGen contract

Use the `$imagegen` skill's built-in `image_gen` tool for every generated reference and animation frame. Stay in built-in mode for normal generation, reference-based derivation, transparency, quality, and file-placement requests. Use the CLI/API fallback only after the user explicitly asks for or confirms that mode. Generate one distinct asset per call; “batch” never means combining different frames into one generation.

Store the set-wide style in `set.json` and copy the exact same object into every member `role.json` so each role remains self-describing:

```json
"renderStyle": {
  "preset": "photorealistic-full-color",
  "prompt": "full-color photorealistic historical character cutout; physically plausible human anatomy; natural skin texture; visible cloth weave, worn leather grain, wood grain, and realistic metal reflections; neutral color-accurate studio lighting; production-quality game character photography",
  "avoid": [
    "monochrome", "grayscale", "sepia", "illustration", "anime",
    "pixel art", "painterly concept art", "cartoon", "toy-like 3D render",
    "motion blur", "depth of field", "film grain", "watermark"
  ],
  "referenceImages": []
}
```

This exact object is the default created by `sprite_tool.py init`. Full-color photorealism is mandatory only when the user does not specify a style. When the user requests another visual style, replace `preset` with a lowercase kebab-case style ID, rewrite `prompt` as one concrete medium/rendering specification, and replace incompatible `avoid` entries. Never silently blend the default with a custom style.

`referenceImages` may contain repo-relative paths to visual style references. Keep identity references separate: front/back establish the character, while `renderStyle.referenceImages` establish medium, surface treatment, palette behavior, and edge treatment. Inspect every local reference with `view_image` before generation and label its role explicitly when calling ImageGen. Reference paths must stay inside the repository and must exist before validation.

Lock `renderStyle` for the entire asset set before approving the first front/back pair. Repeat the full style prompt and avoid list in every later generation. A style change after approval creates a new asset-set ID and invalidates every dependent action/direction frame; never mix styles inside one set.

Use this prompt scaffold for every built-in ImageGen call:

```text
Use case: photorealistic-natural
Asset type: transparent battle-sim soldier sprite frame
Input images: Image 1 front identity reference; Image 2 back identity reference; additional images labeled as style or temporal-pose references
Primary request: render exactly one <role>, <action>, <direction>, frame slot K of 8 using its framePlans entry
Scene/backdrop: genuinely transparent background; no floor or environment
Subject: identity and equipment invariants from role.json; identity.period; identity.region; exact framePlans pose and playback semantics
Style/medium: renderStyle.prompt verbatim
Composition/framing: orthographic quarter-view; full body; fixed scale and foot anchor; 2:3 portrait
Lighting/mood: fixed neutral screen-space lighting; color-accurate materials
Color palette: role palette; full color unless the selected style explicitly says otherwise
Materials/textures: preserve the approved skin, cloth, leather, wood, and metal treatment
Constraints: preserve period and regional accuracy, identity, garment construction, handedness, equipment side/count/size, camera, anchor, style, and alpha; no text; no watermark
Avoid: renderStyle.avoid plus checkerboard, background, floor, baked shadow, clipping, extra equipment, mirrored anatomy
```

For the default style use the exact ImageGen taxonomy slug `photorealistic-natural`. For a user-selected non-photorealistic style use `stylized-concept`. Style references guide generation and are not edit targets. Ask ImageGen for genuine transparency, preserve the alpha channel, inspect the result, and copy an approved project-bound image from the generated-images location into its canonical workspace path.

## Layout and path contract

Group period, region, and style into one stable asset-set ID. Its canonical form is `<region-id>-<period-id>-<style-id>`, using lowercase kebab-case components. The default is `europe-14c-photorealistic-full-color`.

Keep editable sources and shipped output separate:

```text
art/sprites/sets/<asset-set-id>/units/<role>/
├── role.json
├── references/front.png
├── references/back.png
└── frames/<action>/<direction>/frame-0.png ... frame-7.png

web/public/assets/sprites/v2/
├── manifest.json
└── sets/<asset-set-id>/<role>/<action>.png
```

The set root also contains `art/sprites/sets/<asset-set-id>/set.json`, which declares period, region, style, and the role membership list.

Every action has all eight direction directories in the table order. Do not use spaces, capitals, underscores, localized filenames, `final`, or revision suffixes in canonical paths. Keep experiments outside canonical paths.

Each shipped sheet is exactly 1600x2400 RGBA PNG with no gutters:

- 8 columns are the ordered directions above.
- 8 rows are frames `0` through `7`.
- Cell origin is `(directionColumn * 200, frameRow * 300)`.
- Transparent cell margins prevent linear-filter bleeding.

The v2 manifest records the asset set, dimensions, order, per-action playback, simulator binding, and paths. The runtime profile is `web/public/assets/sprites/v2/profiles/<profile>.json`; runtime sheet URLs begin `/assets/sprites/v2/sets/<asset-set-id>/`. Treat runtime profiles, manifests, and sheets as generated; edit the simulator profile, research, or source frames and rerun the relevant command.

Put durable variant identity in the directory path exactly once. Keep leaf filenames semantic and short:

- `front.png` and `back.png` identify the reference kind.
- `frame-0.png` through `frame-7.png` identify the action-specific frame slot.
- `<action>.png` identifies the packed action.
- Do not repeat period, region, style, role, or set ID in a leaf filename when ancestor directories already provide them.

The set directory and `role.json.assetSet.id` must match. The component IDs in `role.json.assetSet` must reconstruct the same value in the order `<region-id>-<period-id>-<style-id>`. Human-readable values remain separate: `identity.period` and `identity.region` are prompt/display values, while `periodId`, `regionId`, and `styleId` are stable path keys.

## Manifest mapping policy

Use JSON as the authoritative lookup from semantic keys to physical files, but do not allow unconstrained arbitrary paths. A fully arbitrary mapping is flexible yet weakens discoverability, makes code review and manual inspection harder, permits duplicate or orphaned files, and turns a missing/corrupt manifest into total loss of asset meaning.

Use this hybrid rule:

1. Keep every source and shipped file under the canonical set/role/action hierarchy above.
2. Let `manifest.json` map `set ID -> declared role -> action -> sheet path` and record period, region, style, and the set-specific role list.
3. Validate that every mapped path is repo-relative, remains under `web/public/assets/sprites/v2/sets/`, and agrees with its semantic keys.
4. Reject missing targets, duplicate target paths, path traversal, case-only aliases, and manifest entries with conflicting set metadata.
5. Allow storage-layout changes only through a manifest schema/version change and migration, not ad hoc per-file paths.

This preserves JSON-driven runtime lookup while keeping assets understandable and recoverable without the manifest.

## Identity contract

Complete `role.json` before reference generation. Specify `renderStyle`; historical period and region; physique/proportions; face/hair/skin; garment layers/cut/colors/wear; armor pieces/material/topology; weapon and tool types/proportions/grip; anatomical handedness; accessory sides; palette; and invariants that may never appear, disappear, swap sides, or change material.

Declare the default path identity explicitly:

```json
"assetSet": {
  "id": "europe-14c-photorealistic-full-color",
  "periodId": "14c",
  "regionId": "europe",
  "styleId": "photorealistic-full-color"
}
```

Store historical context as `identity.period` and `identity.region`. Defaults are `14th century CE` and `Europe`; their stable path IDs are `14c` and `europe`. The user may replace either with a more precise range or location before research/reference generation, but must first define the matching simulator profile and create/use a matching asset-set ID rather than mutating an approved set in place. Repeat both human-readable values in every ImageGen call and require historically plausible garment cuts, armor construction, weapons, tools, dyes, fasteners, and materials for that combination. Do not introduce equipment from another period or region merely because it is visually familiar.

Generate references as the same identity and loadout in the same neutral stance and locked style. Compare layer continuity, straps, hems, scabbards, quivers, shield straps, weapon length, texture treatment, palette behavior, and edge rendering. Approval freezes identity and style; animation may change pose and occlusion, not construction or medium.

Every later image call references both approved base images. Direction expansion also references the same frame slot from nearest approved directions. Do not generate a multi-character sheet as the target; create one isolated character frame per call and pack it deterministically. Never mirror final art because it swaps anatomical equipment and lighting.

## Per-action frame-slot semantics

Every action owns exactly eight frame slots and declares one `actionPlayback` mode. `framePlans` must contain eight concrete, unique pose descriptions whose metadata matches the mode.

### Uniform eight-phase cycles

Use `{"mode":"cycle","variantCount":1,"framesPerVariant":8,"framesPerSecond":8,"loop":true}` for movement, combat, shooting, reloading, and work. `framesPerSecond` is simulator-owned and may differ by action. Bias prevention is mandatory: never ask ImageGen to choose representative poses, because that over-samples salient contact and impact moments.

```text
frame:  0    1    2    3    4    5    6    7    (8 is not stored)
phase: 0/8  1/8  2/8  3/8  4/8  5/8  6/8  7/8  -> 8/8 = next-cycle 0/8
time:   0% 12.5%  25% 37.5%  50% 62.5%  75% 87.5% -> 100%
```

Generate in balanced order `0,4,2,6,1,3,5,7`. Frames 0/4 are opposed half-cycle anchors; 2/6 are bracketed quarter-cycle anchors; odd frames are exact temporal midpoints of their two neighbors. Frame 7 is bracketed by 6 and next-cycle 0. Use this clause for in-betweens: `Render exactly one isolated pose: frame K of 8 at normalized phase K/8. It is the exact temporal midpoint between the supplied phase A and phase B poses. Preserve identity and do not favor, repeat, or jump to either endpoint.`

Make each pose concrete and temporally ordered. Describe limb state, center-of-mass direction, planted foot, weapon/tool phase, cloth follow-through, and recovery. Include anticipation, contact, follow-through, and recovery at their actual samples; never allocate extra frames to impact. Review at one uniform frame duration.

### Idle: four two-frame styles

Use `{"mode":"variant-loop","variantCount":4,"framesPerVariant":2,"framesPerSecond":2,"loop":true}` only for `idle`:

| Rows | Stable style | Playback |
|---|---:|---|
| 0, 1 | 0 | `0 -> 1 -> 0 ...` |
| 2, 3 | 1 | `2 -> 3 -> 2 ...` |
| 4, 5 | 2 | `4 -> 5 -> 4 ...` |
| 6, 7 | 3 | `6 -> 7 -> 6 ...` |

The renderer hashes the stable soldier ID to select one style and retains it. Each pair is one subtle, seamless two-frame wait loop. Make the four styles visibly distinct in stance or watch behavior while preserving identity and loadout. Do not interpolate between pairs, interpret row 2 as following row 1 in time, or randomly reselect a style during playback.

### Dead: eight static variants

Use `{"mode":"static-variants","variantCount":8,"framesPerVariant":1,"framesPerSecond":0,"loop":false}` only for `dead`. Rows 0 through 7 are eight distinct motionless resting poses. They are not phases, have no temporal order, and must never animate or interpolate. The renderer hashes the stable soldier ID once to choose one row. Preserve all role-defining equipment, keep the body and equipment inside the universal cell, and avoid graphic gore unless explicitly requested.

### Direction consistency

For every mode, a frame slot has the same meaning in all eight directions. Direction changes rotate the subject, never time, idle-style identity, or dead-pose identity. Exact duplicate slots are a hard failure. Near duplicates, a skipped cyclic motion arc, indistinguishable idle styles, or duplicate dead silhouettes are visual failures even when pixel validation passes.

## Quality gates

### Reference gate

- Front/back show one identity/loadout with matching camera, lighting, scale, anchor, palette, material, and handedness.
- Clothing, armor, weapons, tools, dyes, and construction are supported by the role research and plausible for the declared period and region; defaults are 14th century CE and Europe.
- Front/back match the declared `renderStyle`; default output is visibly full-color and photorealistic rather than illustrated, monochrome, painterly, or toy-like.
- Garment and strap topology continues around the body.
- User approves the pair before batch generation.

### Timing and action gate

- `actionPlayback` and all eight `framePlans` match: cycle 1x8, idle 4x2, or dead 8x1.
- South/north cycles, idle pairs, or dead variant sets are approved before other directions.
- Cycle pose distribution spans the full motion; impact/contact poses are not overrepresented; frame 7 is distinct and flows to 0.
- Each idle pair loops cleanly, the four idle styles remain distinguishable, and style selection stays fixed per soldier.
- All eight dead slots are distinct, motionless, and selected once per soldier.
- Planted feet stay fixed through contact; anatomy, equipment, and palette do not drift.

### Direction gate

- A frame number means the same phase or stable variant in all directions.
- Rotation is coherent in 45-degree steps.
- East/west are independently generated and retain anatomical sides/lighting.
- Camera elevation, focal scale, and actor scale never change.

### Delivery gate

- `sprite_tool.py validate` returns zero errors.
- `sprite_tool.py assemble` creates exact 1600x2400 output and a manifest entry.
- Inspect edges over black and white for halos.
- Preview at 100% and expected in-game scale using the declared per-action playback mode.
- Reject missing, duplicate, clipped, checkerboard, opaque-background, mirrored, or timing-biased frames.

## Legacy boundary

Files directly under `web/public/assets/sprites/<role>/<action>.png` use the legacy 1536x1024, 8-direction x 4-frame format. Do not overwrite them or claim compliance. New work belongs under `art/sprites/sets/<asset-set-id>/units/` and `web/public/assets/sprites/v2/sets/<asset-set-id>/`. The renderer reads the v2 runtime profile and manifest and falls back to legacy imagery only while a required v2 sheet is absent.
