---
name: generate-battle-sprites
description: Research historically plausible soldier appearance on the web, turn simulator-owned period/region/role/action definitions into ImageGen-ready role definitions, then generate, extend, validate, assemble, and register battle-sim soldier sprites. Use for simulator visual-profile import, cited clothing/armor/weapon research, front/back references, full-color photorealistic or custom-style quarter-view characters, evenly timed 8-direction actions, 4x2 idle variants, 8 static dead variants, transparent PNG cleanup, sprite-sheet packing, or v2 manifest/runtime integration. Do not use for terrain, UI icons, effects, or non-character art.
---

# Generate Battle Sprites

Build assets from the simulator's semantic definition and keep historical visual research outside simulation logic. Never infer roles from filenames or comments, generate a complete sheet in one prompt, or treat an unvalidated image as finished. Use a solid `#00ff00` chroma-key background for every raw ImageGen output; never use that color in the subject, and never accept a checkerboard background.

## Load the contracts

Read [references/asset-contract.md](references/asset-contract.md) completely before researching, editing definitions, generating, packing, or validating. It is authoritative for ownership, schemas, dimensions, directions, paths, alpha, identity, evidence, playback, and runtime ingestion.

## Start from the simulator definition

Treat `data/visual-profiles/<profile>.toml` as simulator-owned. It defines period, region, role IDs/descriptions, numeric troop-type indices, applicable actions, playback modes, and the complete simulator-state-to-action mapping. Do not add a role, action, or historical context in the asset workspace when it is absent from that profile; request or implement the simulator definition first.

Import the default 14th-century Europe profile:

```bash
python3 .agents/skills/generate-battle-sprites/scripts/sprite_tool.py import-simulator \
  --repo . --profile medieval_western
```

This creates or synchronizes:

- `art/sprites/sets/europe-14c-photorealistic-full-color/set.json`
- one `role.json` and `research.json` per simulator role
- `web/public/assets/sprites/v2/profiles/medieval_western.json` for runtime ingestion

Use `init` only for a user-requested standalone experiment that deliberately has no simulator profile. Simulator-bound production work must use `import-simulator`.

## Research the visual definition on the web

For every `research.json` with `status: needs-research`:

1. Use web browsing to run the supplied queries plus narrower role/equipment queries. Search in the profile's exact period and region; do not silently substitute a different century or locality.
2. Prefer primary or authoritative evidence: museum object records, digitized dated manuscripts/effigies, archaeological reports, national collections, university publications, and peer-reviewed scholarship. Use at least two independent domains. Do not use image search thumbnails, shops, unsourced blogs, AI summaries, or reenactor pages as sole evidence.
3. Record each durable URL, title, publisher, access date, and a concise claim. Cover `silhouette`, `garments`, `armor`, `weapons`, `accessories`, and `palette`. Mark confidence `high`, `medium`, or `low`; distinguish direct evidence from a cautious synthesis.
4. Resolve conflicts explicitly in `researchNotes`. Prefer narrower date/place evidence. Do not invent missing equipment for visual drama.
5. Synthesize only image-relevant facts into `visualDefinition`: silhouette, garment construction/layers, armor topology, weapon dimensions/grip, handedness/equipment sides, plausible palette, and immutable invariants.
6. Complete every `framePlans` pose with role-specific mechanics. A mounted role must describe rider and horse gait rather than biped footfalls; a spear, bow, crossbow, tool, or shield action must follow its researched grip and mechanism. Keep simulator action meaning unchanged. Set `actionPlanStatus` to `ready` only after all eight slots of every action are concrete and unique.
7. Set `status` to `ready`, then apply and validate:

   ```bash
   python3 .agents/skills/generate-battle-sprites/scripts/sprite_tool.py apply-research \
     --repo . --set <asset-set-id> --role <role>
   python3 .agents/skills/generate-battle-sprites/scripts/sprite_tool.py validate \
     --repo . --set <asset-set-id> --role <role> --definition-only
   ```

`apply-research` must fail on missing topics, invalid citations, fewer than two independent domains, incomplete visual definitions, or a simulator/profile mismatch. Web evidence informs appearance only; it must never rewrite simulator roles, actions, state mappings, or troop-type indices.

## Generate through approval gates

1. Lock `set.json`, researched `role.json`, and the set-wide style. Default to `photorealistic-full-color`; only create a new set ID for a user-selected style.
2. Use the `$imagegen` skill and built-in `image_gen` mode. Generate `references/front.png` (`south`) and `references/back.png` (`north`) separately as neutral-pose images on a uniform solid `#00ff00` background. Do not request or accept a checkerboard, white, gray, black, gradient, floor, or scenery background. Inspect local inputs before every edit/derivation. Run the deterministic chroma-key pass below before placing any result at a canonical path, then obtain approval for the pair.
   
   ```bash
   python3 .agents/skills/generate-battle-sprites/scripts/chroma_key_green.py \
     --input <raw-imagegen-output.png> \
     --output <canonical-or-staging-output.png>
   ```

   The pass must replace exact `#00ff00` pixels with `A=0, RGB=0,0,0`; it must fail when no key pixels are present. Keep the raw generated image outside canonical paths. A subject pixel may not use `#00ff00`, including cloth, metal reflections, effects, or antialiased interior details.
3. Validate the definition and references with `validate --references-only`.
4. Generate one action and one isolated image per ImageGen call. Attach both identity references every time. Generate and approve the complete `south` direction first as the first temporal anchor. After `south` passes its eight-frame visual and contract gates, dispatch exactly seven subagents in parallel, one per remaining direction: `southeast`, `east`, `northeast`, `north`, `northwest`, `west`, and `southwest`. Do not finalize any direction from `south` alone: `north` must become the second temporal anchor before the other six directions can be approved. Never mirror final art.

### Parallelize the seven post-south directions

After `south` is complete, use seven independent subagents concurrently rather than generating the remaining directions sequentially:

- Assign exactly one direction to each subagent: `southeast`, `east`, `northeast`, `north`, `northwest`, `west`, or `southwest`.
- Give every subagent the same task-local inputs: the fully validated `role.json` and `research.json`, approved `references/front.png` and `references/back.png`, the approved `south` frames `0..7`, the action's eight frame plans, the locked render style, and the chroma-key/normalization commands.
- Treat `north` as the second-anchor worker. If approved `north` frames already exist, broadcast them with `south` immediately. Otherwise, let the `north` worker stage and validate its frames first; the other six workers may stage experiments but must not approve or place canonical frames until `north` passes its eight-frame visual and contract gates. Then broadcast the approved `north` frames `0..7` to all six workers.
- Keep the subagents independent after the anchor barrier. Every non-anchor direction must use the same-slot `south` and `north` frames plus both identity references; it must not read or depend on another subagent's staging output. No direction may use only `south` as its pose reference.
- Use the balanced frame order `0,4,2,6,1,3,5,7` inside every subagent. Each subagent owns only its direction directory and must write raw outputs to its own staging area before placing approved RGBA frames at that direction's canonical path.
- Require each subagent to perform its own per-direction visual review, chroma-key pass, normalization, `validate_frame` checks, and exact-duplicate check. It must return the direction, generated raw paths, canonical frame paths, rejected/regenerated frames, and validation results.
- Do not let subagents edit `set.json`, `role.json`, `research.json`, the v2 manifest, runtime profiles, or the packed sheet. Only the parent agent may perform the join, cross-direction review, `validate --action`, `assemble`, and manifest/runtime registration.
- If a subagent fails, retry only that direction after preserving the other six results. Do not assemble or register until all seven direction reports are successful and all 64 canonical frames pass the action validator.
- At the join step, compare the same frame slot across directions for identity, scale, foot anchor, equipment side, lighting, and temporal meaning. Repair only the failing direction/frame, then run the final action-wide validation and packing once.

### Lock and repair cross-direction pose continuity

Use this procedure whenever `south` and `north` have coherent frame divisions but another direction drifts into a different action or pose family:

1. Define the approved `south/frame-K.png` and `north/frame-K.png` pair as the temporal pose lock for every slot `K`. A direction change may alter yaw, visible side, foreshortening, and occlusion only; it may not reinterpret the action.
2. Build a pose-lock packet containing both identity references, the same-slot `south` and `north` frames, the eight frame plans, the anchor `(100,276)`, equipment-side invariants, and the locked lighting/style. Attach the packet to every regeneration call. Five references fit the ImageGen limit: front, back, south-K, north-K, and the direction's approved neighboring frame when an in-between is being repaired.
3. Regenerate even anchor frames in the order `0,4,2,6` first. For each even slot, preserve the planted foot, center-of-mass direction, torso/hip lean, sword arm and grip, blade phase/angle, scabbard side, cloth follow-through, silhouette proportions, and timing from the same-slot `south`/`north` pair. Generate odd slots `1,3,5,7` only as exact temporal midpoints between adjacent approved even frames in that same direction, while retaining the same-slot pose lock.
4. Reject a frame when it introduces a new attack phase, changes the planted foot or hand, sheathes/swaps the weapon, changes equipment sides, alters the silhouette scale, or otherwise looks like a different motion viewed from another yaw. Also reject leftover green, checkerboard pixels, anchor drift, lighting drift, and exact duplicates.
5. For an existing bad direction, move its current frames to a recoverable quarantine/staging directory; do not edit, mirror, or mix them into the repair. Keep approved `south` and `north`, regenerate the affected direction from the pose-lock packet, and rerun per-frame plus action-wide validation before assembly. If most frames in a direction drift, regenerate the whole direction rather than mixing two motion families; if only isolated frames drift, regenerate only those slots with their neighboring approved even frames.
6. Before joining, inspect a contact sheet grouped by frame slot, not only by direction. Confirm that all eight directions show the same foot phase, weapon phase, body lean, scale, anchor, handedness/equipment sides, and action meaning. Do not assemble or register the action until every direction passes this cross-direction review.
5. Follow the action's `actionPlayback`:
   - `cycle` (`1x8`): sample exact phases `k/8`; generate in balanced order `0,4,2,6,1,3,5,7`. Odd frames are exact temporal midpoints of adjacent approved even phases.
   - `variant-loop` (`idle`, `4x2`): rows `0-1`, `2-3`, `4-5`, `6-7` are four visibly distinct idle styles. Within each pair, generate phase `0/2` then `1/2` as a subtle two-frame loop. Do not interpolate between different styles.
   - `static-variants` (`dead`, `8x1`): rows `0..7` are eight distinct motionless poses, not a timeline. Generate and review each independently; never animate or interpolate them.
6. Keep a frame number's meaning identical across all eight directions. Attach the same slot from nearest approved directions when rotating it.
7. Apply `chroma_key_green.py` to every raw frame before resizing or normalization, then normalize every frame to the contract without per-frame reframing, equipment repainting, background compositing, checkerboard pixels, or pose duplication. Validate the resulting straight-alpha RGBA PNG; never key a checkerboard after the fact.
8. Validate and pack each action:

   ```bash
   python3 .agents/skills/generate-battle-sprites/scripts/sprite_tool.py validate \
     --repo . --set <asset-set-id> --role <role> --action <action>
   python3 .agents/skills/generate-battle-sprites/scripts/sprite_tool.py assemble \
     --repo . --set <asset-set-id> --role <role> --action <action>
   ```

9. Review individual frames and the 1600x2400 sheet at 100% and in-game scale. Run final role validation. Report research sources, created paths, playback modes, validation results, and any missing runtime actions.

## Use ImageGen consistently

- Use built-in `image_gen` for all generated raster images and reference-derived poses. Use CLI/API fallback only when the user explicitly requests or confirms it.
- Generate exactly one distinct reference/frame per call and request a uniform solid `#00ff00` chroma-key background. Do not request genuine transparency from ImageGen, and never accept a checkerboard as an alpha substitute. Preserve approved source frames; never overwrite one without explicit authorization.
- Build every prompt from `role.json`: researched period/region, identity, `renderStyle`, direction, exact frame-plan entry, alpha, and invariants. Repeat these constraints on every call.
- Use `photorealistic-natural` for the default full-color photorealistic style and `stylized-concept` for a selected non-photorealistic style.
- Repair identity/style failures at the front/back gate. Repair an incorrect cycle key pose before regenerating dependent in-betweens. Regenerate a failed idle phase only within its two-frame style; regenerate a failed dead pose independently.
- Built-in outputs live outside the project by default. Run the chroma-key script, inspect the result for leftover green or checkerboard pixels, then copy each approved transparent output into its canonical workspace path; never leave a project-referenced asset only in generated-images storage.

## Enforce completion

Call an action complete only when all 64 paths exist, its eight frame slots match its declared playback mode, no exact duplicates exist, PNG validation passes, the sheet is assembled, the v2 manifest contains path and playback metadata, and visual review passes. The renderer chooses idle style and dead pose once per soldier with deterministic pseudo-random selection. Legacy 8x4 sheets are comparison inputs only.
