---
name: generate-battle-sprites
description: Research historically plausible soldier appearance on the web, turn simulator-owned period/region/role/action definitions into ImageGen-ready role definitions, then generate, extend, validate, assemble, and register battle-sim soldier sprites. Use for simulator visual-profile import, cited clothing/armor/weapon research, front/back references, full-color photorealistic or custom-style quarter-view characters, evenly timed 8-direction actions, 4x2 idle variants, 8 static dead variants, transparent PNG cleanup, sprite-sheet packing, or v2 manifest/runtime integration. Do not use for terrain, UI icons, effects, or non-character art.
---

# Generate Battle Sprites

Build assets from the simulator's semantic definition and keep historical visual research outside simulation logic. Never infer roles from filenames or comments, generate a complete sheet in one prompt, or treat an unvalidated image as finished.

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
2. Use the `$imagegen` skill and built-in `image_gen` mode. Generate `references/front.png` (`south`) and `references/back.png` (`north`) separately as neutral-pose RGBA images. Inspect local inputs before every edit/derivation. Copy approved project assets into canonical paths and obtain approval for the pair.
3. Validate the definition and references with `validate --references-only`.
4. Generate one action and one isolated image per ImageGen call. Attach both identity references every time. Generate south/north first, review them, then expand in this order: `southeast`, `southwest`, `northeast`, `northwest`, `east`, `west`. Never mirror final art.
5. Follow the action's `actionPlayback`:
   - `cycle` (`1x8`): sample exact phases `k/8`; generate in balanced order `0,4,2,6,1,3,5,7`. Odd frames are exact temporal midpoints of adjacent approved even phases.
   - `variant-loop` (`idle`, `4x2`): rows `0-1`, `2-3`, `4-5`, `6-7` are four visibly distinct idle styles. Within each pair, generate phase `0/2` then `1/2` as a subtle two-frame loop. Do not interpolate between different styles.
   - `static-variants` (`dead`, `8x1`): rows `0..7` are eight distinct motionless poses, not a timeline. Generate and review each independently; never animate or interpolate them.
6. Keep a frame number's meaning identical across all eight directions. Attach the same slot from nearest approved directions when rotating it.
7. Normalize every frame to the contract without per-frame reframing, equipment repainting, background compositing, or pose duplication.
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
- Generate exactly one distinct reference/frame per call and request genuine transparency. Preserve approved source frames; never overwrite one without explicit authorization.
- Build every prompt from `role.json`: researched period/region, identity, `renderStyle`, direction, exact frame-plan entry, alpha, and invariants. Repeat these constraints on every call.
- Use `photorealistic-natural` for the default full-color photorealistic style and `stylized-concept` for a selected non-photorealistic style.
- Repair identity/style failures at the front/back gate. Repair an incorrect cycle key pose before regenerating dependent in-betweens. Regenerate a failed idle phase only within its two-frame style; regenerate a failed dead pose independently.
- Built-in outputs live outside the project by default. Copy each approved output into its canonical workspace path; never leave a project-referenced asset only in generated-images storage.

## Enforce completion

Call an action complete only when all 64 paths exist, its eight frame slots match its declared playback mode, no exact duplicates exist, PNG validation passes, the sheet is assembled, the v2 manifest contains path and playback metadata, and visual review passes. The renderer chooses idle style and dead pose once per soldier with deterministic pseudo-random selection. Legacy 8x4 sheets are comparison inputs only.
