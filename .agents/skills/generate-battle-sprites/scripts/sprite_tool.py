#!/usr/bin/env python3
"""Initialize, validate, and assemble battle-sim v2 soldier sprites."""

from __future__ import annotations

import argparse
import binascii
import datetime as dt
import hashlib
import json
import re
import struct
import sys
import tomllib
import zlib
from pathlib import Path
from urllib.parse import urlparse


CELL_W, CELL_H, FRAME_COUNT = 200, 300, 8
DIRECTIONS = (
    "south", "southeast", "east", "northeast",
    "north", "northwest", "west", "southwest",
)
ID_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
PNG_SIG = b"\x89PNG\r\n\x1a\n"
DEFAULT_SET_ID = "europe-14c-photorealistic-full-color"
DEFAULT_PERIOD_ID = "14c"
DEFAULT_REGION_ID = "europe"
SIMULATION_STATES = (
    "idle", "marching", "repositioning", "advancing", "charging", "engaged",
    "shooting", "reloading", "wavering", "broken", "rallying", "working",
    "downed", "dead",
)
RESEARCH_CATEGORIES = (
    "silhouette", "garments", "armor", "weapons", "accessories", "palette",
)
ACTION_CYCLE_POSES = {
    "idle": (
        "neutral stance; inhale begins; weight centered over both feet",
        "chest and cloth rise slightly; hands retain the approved grip",
        "inhale high midpoint; shoulders at the top of their subtle rise",
        "breath settles toward peak; equipment inertia trails slightly",
        "exhale begins; torso starts its controlled return",
        "chest and cloth fall slightly; weight remains centered",
        "exhale low midpoint; shoulders at the bottom of their subtle fall",
        "return toward neutral; equipment settles into next-cycle phase 0",
    ),
    "walk": (
        "left heel contact; right toe leaves the ground; weight moving forward",
        "left leg compresses under weight; right foot begins its forward swing",
        "left leg passes under torso; right knee advances; center of mass rises",
        "left heel rises; right lower leg reaches forward; equipment follows through",
        "right heel contact; left toe leaves the ground; weight moving forward",
        "right leg compresses under weight; left foot begins its forward swing",
        "right leg passes under torso; left knee advances; center of mass rises",
        "right heel rises; left lower leg reaches forward; equipment returns toward phase 0",
    ),
    "run": (
        "left foot contact; right leg trailing; torso committed forward",
        "left leg compresses; right knee drives forward; equipment lags",
        "airborne passing phase; left foot leaves ground; right thigh advances",
        "right foot reaches for landing; arms and carried equipment counter-swing",
        "right foot contact; left leg trailing; torso committed forward",
        "right leg compresses; left knee drives forward; equipment lags",
        "airborne passing phase; right foot leaves ground; left thigh advances",
        "left foot reaches for landing; arms and equipment return toward phase 0",
    ),
    "combat": (
        "guarded ready position; weight balanced; weapon at start of attack cycle",
        "anticipation; weight loads rearward; weapon draws back without changing grip",
        "attack begins; hips and shoulders rotate; lead foot remains planted",
        "weapon accelerates toward contact; center of mass moves through the strike",
        "contact phase; full extension without over-sampling impact",
        "follow-through; weapon passes contact line; cloth and straps trail",
        "recovery begins; weight returns; weapon withdraws along a safe arc",
        "guard nearly restored; equipment settles continuously toward phase 0",
    ),
    "shoot": (
        "ready with projectile weapon aligned; ammunition prepared",
        "raise and acquire target; feet remain planted",
        "draw or aim develops to the midpoint; shoulders engage",
        "full aim immediately before release; weapon remains correctly gripped",
        "release or discharge; single contact instant without an extra hold",
        "early follow-through; recoil or bow response resolves",
        "weapon lowers slightly; firing hand begins recovery",
        "return to ready; equipment and posture continue toward phase 0",
    ),
    "reload": (
        "reload begins from post-shot ready; support hand moves to ammunition",
        "reach and acquire ammunition or loading component",
        "bring ammunition to the weapon; torso remains stable",
        "seat, nock, or place the ammunition halfway through the procedure",
        "loading action reaches its mechanically complete midpoint",
        "secure or tension the weapon; hands follow the approved mechanism",
        "return weapon toward firing-ready position",
        "reload finishes; grip and posture settle continuously toward phase 0",
    ),
    "work": (
        "tool ready at neutral work position; weight centered",
        "anticipation; tool draws back and weight loads away from target",
        "work stroke begins; hips and shoulders drive the tool",
        "tool accelerates toward the work point",
        "contact with the work point; single impact instant",
        "follow-through; tool passes contact while stance absorbs force",
        "tool withdraws; weight returns toward neutral",
        "ready position nearly restored; continuous transition toward phase 0",
    ),
}
IDLE_VARIANT_POSES = (
    (
        "variant 0 steady guard; inhale with weight evenly planted",
        "variant 0 steady guard; exhale and settle back without changing stance",
    ),
    (
        "variant 1 alert watch; head and gaze begin a small scan to anatomical left",
        "variant 1 alert watch; head and gaze return to center while feet stay planted",
    ),
    (
        "variant 2 equipment-ready stance; hands make a subtle grip check",
        "variant 2 equipment-ready stance; grip and shoulders relax to the starting pose",
    ),
    (
        "variant 3 rested guard; weight shifts subtly toward the anatomical left foot",
        "variant 3 rested guard; weight returns toward center with no step",
    ),
)
DEAD_VARIANT_POSES = (
    "variant 0 motionless supine fall; limbs and equipment settled naturally",
    "variant 1 motionless prone fall; face turned safely aside; equipment settled",
    "variant 2 motionless fall on anatomical left side; compact readable silhouette",
    "variant 3 motionless fall on anatomical right side; compact readable silhouette",
    "variant 4 motionless supine diagonal sprawl; no active muscle tension",
    "variant 5 motionless prone diagonal sprawl; weapon resting separately but still present",
    "variant 6 motionless curled side fall; role-defining equipment remains visible",
    "variant 7 motionless collapsed kneel-to-side fall; body fully at rest on ground",
)
DEFAULT_RENDER_STYLE = {
    "preset": "photorealistic-full-color",
    "prompt": (
        "full-color photorealistic historical character cutout; physically plausible human anatomy; "
        "natural skin texture; visible cloth weave, worn leather grain, wood grain, and realistic metal "
        "reflections; neutral color-accurate studio lighting; production-quality game character photography"
    ),
    "avoid": [
        "monochrome", "grayscale", "sepia", "illustration", "anime", "pixel art",
        "painterly concept art", "cartoon", "toy-like 3D render", "motion blur",
        "depth of field", "film grain", "watermark",
    ],
    "referenceImages": [],
}


class ContractError(Exception):
    pass


def check_id(value: str, kind: str) -> None:
    if not ID_RE.fullmatch(value):
        raise ContractError(f"invalid {kind} ID {value!r}; use lowercase kebab-case")


def set_root(repo: Path, set_id: str) -> Path:
    return repo / "art" / "sprites" / "sets" / set_id


def source_root(repo: Path, set_id: str, role: str) -> Path:
    return set_root(repo, set_id) / "units" / role


def write_json(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def simulation_profile_path(repo: Path, profile_id: str) -> Path:
    return repo / "data" / "visual-profiles" / f"{profile_id}.toml"


def load_simulation_profile(repo: Path, profile_id: str) -> dict:
    check_id(profile_id.replace("_", "-"), "simulation profile")
    path = simulation_profile_path(repo, profile_id)
    if not path.is_file():
        raise ContractError(f"missing simulator-owned visual profile {path}")
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise ContractError(f"cannot read {path}: {exc}") from exc
    if data.get("schema_version") != 1 or data.get("id") != profile_id:
        raise ContractError(f"{path}: require schema_version=1 and id={profile_id!r}")
    context = data.get("historical_context")
    defaults = data.get("asset_defaults")
    if not isinstance(context, dict) or not isinstance(defaults, dict):
        raise ContractError(f"{path}: historical_context and asset_defaults are required tables")
    for key, kind in (("period_id", "period"), ("region_id", "region")):
        value = context.get(key)
        if not isinstance(value, str):
            raise ContractError(f"{path}: historical_context.{key} must be a string")
        check_id(value, kind)
    for key in ("period_label", "region_label"):
        if not isinstance(context.get(key), str) or not context[key].strip():
            raise ContractError(f"{path}: historical_context.{key} must be non-empty")
    style_id = defaults.get("style_id")
    if not isinstance(style_id, str):
        raise ContractError(f"{path}: asset_defaults.style_id must be a string")
    check_id(style_id, "style")

    actions = data.get("actions")
    if not isinstance(actions, list) or not actions:
        raise ContractError(f"{path}: define at least one [[actions]] entry")
    action_ids: set[str] = set()
    for action in actions:
        if not isinstance(action, dict) or not isinstance(action.get("id"), str):
            raise ContractError(f"{path}: every action needs a string id")
        action_id = action["id"]
        check_id(action_id, "action")
        if action_id in action_ids:
            raise ContractError(f"{path}: duplicate action {action_id!r}")
        if not isinstance(action.get("description"), str) or not action["description"].strip():
            raise ContractError(f"{path}: action {action_id!r} needs a description")
        playback = action.get("playback")
        shape = (action.get("variant_count"), action.get("frames_per_variant"), action.get("loop"))
        expected_shape = {
            "cycle": (1, 8, True),
            "variant-loop": (4, 2, True),
            "static-variants": (8, 1, False),
        }.get(playback)
        if expected_shape is None or shape != expected_shape:
            raise ContractError(
                f"{path}: action {action_id!r} playback must be cycle=1x8 loop, "
                "variant-loop=4x2 loop, or static-variants=8x1 non-loop"
            )
        fps = action.get("frames_per_second")
        if not isinstance(fps, int) or fps < 0 or (playback == "static-variants") != (fps == 0):
            raise ContractError(
                f"{path}: action {action_id!r} needs positive integer frames_per_second, "
                "or 0 only for static-variants"
            )
        if playback == "cycle" and action_id not in ACTION_CYCLE_POSES:
            raise ContractError(f"{path}: no uniform 8-phase plan is defined for action {action_id!r}")
        if playback == "variant-loop" and action_id != "idle":
            raise ContractError(f"{path}: variant-loop is currently reserved for idle")
        if playback == "static-variants" and action_id != "dead":
            raise ContractError(f"{path}: static-variants is currently reserved for dead")
        action_ids.add(action_id)

    roles = data.get("roles")
    if not isinstance(roles, list) or not roles:
        raise ContractError(f"{path}: define at least one [[roles]] entry")
    role_ids: set[str] = set()
    troop_ids: set[str] = set()
    troop_indices: set[int] = set()
    for role in roles:
        if not isinstance(role, dict):
            raise ContractError(f"{path}: every role must be a table")
        role_id, troop_id, troop_index = role.get("id"), role.get("troop_type_id"), role.get("troop_type_index")
        if not isinstance(role_id, str) or not isinstance(troop_id, str) or not isinstance(troop_index, int):
            raise ContractError(f"{path}: every role needs id, troop_type_id, and integer troop_type_index")
        check_id(role_id, "role")
        if not re.fullmatch(r"[a-z0-9]+(?:_[a-z0-9]+)*", troop_id):
            raise ContractError(f"{path}: invalid troop_type_id {troop_id!r}; use lowercase snake_case")
        if role_id in role_ids or troop_id in troop_ids or troop_index in troop_indices or troop_index < 0:
            raise ContractError(f"{path}: role IDs, troop type IDs, and nonnegative indices must be unique")
        for key in ("name_ja", "name_en", "description"):
            if not isinstance(role.get(key), str) or not role[key].strip():
                raise ContractError(f"{path}: role {role_id!r} needs {key}")
        role_actions = role.get("actions")
        if (
            not isinstance(role_actions, list) or not role_actions
            or len(role_actions) != len(set(role_actions))
            or any(action not in action_ids for action in role_actions)
        ):
            raise ContractError(f"{path}: role {role_id!r} actions must be unique declared action IDs")
        mapping = role.get("state_actions")
        if not isinstance(mapping, dict) or set(mapping) != set(SIMULATION_STATES):
            missing = sorted(set(SIMULATION_STATES) - set(mapping or {}))
            extra = sorted(set(mapping or {}) - set(SIMULATION_STATES))
            raise ContractError(f"{path}: role {role_id!r} state_actions mismatch; missing={missing}, extra={extra}")
        if any(action not in role_actions for action in mapping.values()):
            raise ContractError(f"{path}: role {role_id!r} maps a state to an undeclared action")
        role_ids.add(role_id)
        troop_ids.add(troop_id)
        troop_indices.add(troop_index)
    return data


def frame_plan(action: dict) -> list[dict]:
    action_id, playback = action["id"], action["playback"]
    if playback == "cycle":
        return [
            {"frame": frame, "phase": f"{frame}/8", "pose": pose}
            for frame, pose in enumerate(ACTION_CYCLE_POSES[action_id])
        ]
    if playback == "variant-loop":
        return [
            {
                "frame": variant * 2 + phase,
                "variant": variant,
                "phase": f"{phase}/2",
                "pose": IDLE_VARIANT_POSES[variant][phase],
            }
            for variant in range(4)
            for phase in range(2)
        ]
    return [
        {"frame": variant, "variant": variant, "phase": "static", "pose": pose}
        for variant, pose in enumerate(DEAD_VARIANT_POSES)
    ]


def role_research_template(profile: dict, role: dict) -> dict:
    context = profile["historical_context"]
    role_name = role["name_en"]
    action_definitions = {action["id"]: action for action in profile["actions"]}
    empty_frame_plans = {}
    for action in role["actions"]:
        entries = frame_plan(action_definitions[action])
        empty_frame_plans[action] = [{**entry, "pose": ""} for entry in entries]
    return {
        "schemaVersion": 2,
        "status": "needs-research",
        "simulationProfile": profile["id"],
        "role": role["id"],
        "brief": {
            "period": context["period_label"],
            "region": context["region_label"],
            "simulationDescription": role["description"],
            "actions": role["actions"],
        },
        "requiredTopics": list(RESEARCH_CATEGORIES),
        "suggestedQueries": [
            f"{context['period_label']} {context['region_label']} {role_name} clothing armor museum",
            f"{context['period_label']} {role_name} weapons manuscript archaeology",
            f"site:metmuseum.org {context['period_label']} {role_name} armor weapon",
        ],
        "sources": [],
        "claims": [],
        "visualDefinition": {
            "silhouette": "",
            "garments": "",
            "armor": "",
            "weapons": "",
            "handedness": "",
            "palette": "",
            "invariants": [],
        },
        "actionPlanStatus": "needs-review",
        "framePlans": empty_frame_plans,
        "researchNotes": "",
    }


def load_set(repo: Path, set_id: str) -> dict:
    path = set_root(repo, set_id) / "set.json"
    if not path.is_file():
        raise ContractError(f"missing {path}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ContractError(f"cannot read {path}: {exc}") from exc
    if data.get("schemaVersion") != 1 or data.get("id") != set_id:
        raise ContractError(f"{path}: require schemaVersion 1 and id {set_id!r}")
    period, region, style_id = data.get("period"), data.get("region"), data.get("styleId")
    if not isinstance(period, dict) or not isinstance(region, dict):
        raise ContractError(f"{path}: period and region must be objects")
    for value, kind in (
        (period.get("id"), "period"), (region.get("id"), "region"), (style_id, "style")
    ):
        if not isinstance(value, str):
            raise ContractError(f"{path}: {kind} ID must be a string")
        check_id(value, kind)
    if not isinstance(period.get("label"), str) or not period["label"].strip():
        raise ContractError(f"{path}: period.label must be non-empty")
    if not isinstance(region.get("label"), str) or not region["label"].strip():
        raise ContractError(f"{path}: region.label must be non-empty")
    expected_set = f"{region['id']}-{period['id']}-{style_id}"
    if expected_set != set_id:
        raise ContractError(f"{path}: id must equal <region.id>-<period.id>-<styleId> ({expected_set})")
    roles = data.get("roles")
    if not isinstance(roles, list) or not roles or len(roles) != len(set(roles)):
        raise ContractError(f"{path}: roles must be a non-empty unique array")
    for role in roles:
        if not isinstance(role, str):
            raise ContractError(f"{path}: role IDs must be strings")
        check_id(role, "role")
    style = data.get("renderStyle")
    if not isinstance(style, dict) or style.get("preset") != style_id:
        raise ContractError(f"{path}: renderStyle.preset must equal styleId")
    return data


def load_role(repo: Path, set_id: str, role: str) -> dict:
    set_data = load_set(repo, set_id)
    if role not in set_data["roles"]:
        raise ContractError(f"{set_root(repo, set_id) / 'set.json'}: undeclared role {role!r}")
    path = source_root(repo, set_id, role) / "role.json"
    if not path.is_file():
        raise ContractError(f"missing {path}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ContractError(f"cannot read {path}: {exc}") from exc
    schema_version = data.get("schemaVersion")
    if schema_version not in (1, 2) or data.get("role") != role:
        raise ContractError(f"{path}: require schemaVersion 1 or 2 and role {role!r}")
    asset_set = data.get("assetSet")
    if not isinstance(asset_set, dict):
        raise ContractError(f"{path}: assetSet must be an object")
    components = ("id", "periodId", "regionId", "styleId")
    if any(not isinstance(asset_set.get(key), str) for key in components):
        raise ContractError(f"{path}: assetSet must contain string id, periodId, regionId, and styleId")
    for key in components:
        check_id(asset_set[key], f"asset set {key}")
    expected_set = f"{asset_set['regionId']}-{asset_set['periodId']}-{asset_set['styleId']}"
    if asset_set["id"] != set_id or expected_set != set_id:
        raise ContractError(
            f"{path}: assetSet.id and directory must equal <regionId>-<periodId>-<styleId> ({expected_set})"
        )
    actions = data.get("actions")
    if not isinstance(actions, list) or not actions or len(actions) != len(set(actions)):
        raise ContractError(f"{path}: actions must be a non-empty unique array")
    for action in actions:
        if not isinstance(action, str):
            raise ContractError(f"{path}: action IDs must be strings")
        check_id(action, "action")

    simulation = data.get("simulation")
    if simulation is not None:
        if not isinstance(simulation, dict):
            raise ContractError(f"{path}: simulation must be an object")
        for key in ("profile", "troopTypeId", "description"):
            if not isinstance(simulation.get(key), str) or not simulation[key].strip():
                raise ContractError(f"{path}: simulation.{key} must be non-empty")
        if not isinstance(simulation.get("troopTypeIndex"), int) or simulation["troopTypeIndex"] < 0:
            raise ContractError(f"{path}: simulation.troopTypeIndex must be a nonnegative integer")
        state_actions = simulation.get("stateActions")
        if not isinstance(state_actions, dict) or set(state_actions) != set(SIMULATION_STATES):
            raise ContractError(f"{path}: simulation.stateActions must map every simulator state exactly once")
        if any(action not in actions for action in state_actions.values()):
            raise ContractError(f"{path}: simulation.stateActions maps to an undeclared role action")

    identity = data.get("identity")
    fields = (
        "period", "region", "silhouette", "garments", "armor",
        "weapons", "handedness", "palette", "invariants",
    )
    if not isinstance(identity, dict):
        raise ContractError(f"{path}: identity must be an object")
    missing = [field for field in fields if not identity.get(field)]
    if missing:
        raise ContractError(f"{path}: complete identity fields: {', '.join(missing)}")
    if identity["period"] != set_data["period"]["label"] or identity["region"] != set_data["region"]["label"]:
        raise ContractError(f"{path}: identity period/region must match set.json")

    style = data.get("renderStyle")
    if not isinstance(style, dict):
        raise ContractError(f"{path}: renderStyle must be an object")
    preset = style.get("preset")
    if not isinstance(preset, str):
        raise ContractError(f"{path}: renderStyle.preset must be a string")
    check_id(preset, "render style")
    prompt = style.get("prompt")
    if not isinstance(prompt, str) or not prompt.strip():
        raise ContractError(f"{path}: renderStyle.prompt must be a non-empty style specification")
    avoid = style.get("avoid")
    if not isinstance(avoid, list) or any(not isinstance(item, str) or not item.strip() for item in avoid):
        raise ContractError(f"{path}: renderStyle.avoid must be an array of non-empty strings")
    if len({item.strip().casefold() for item in avoid}) != len(avoid):
        raise ContractError(f"{path}: renderStyle.avoid contains duplicates")
    references = style.get("referenceImages")
    if not isinstance(references, list) or any(not isinstance(item, str) or not item for item in references):
        raise ContractError(f"{path}: renderStyle.referenceImages must be an array of repo-relative paths")
    if len(references) != len(set(references)):
        raise ContractError(f"{path}: renderStyle.referenceImages contains duplicates")
    if style["preset"] != asset_set["styleId"]:
        raise ContractError(f"{path}: renderStyle.preset must equal assetSet.styleId")
    if style != set_data["renderStyle"]:
        raise ContractError(f"{path}: renderStyle must match set.json for every role in the set")
    resolved_repo = repo.resolve()
    for reference in references:
        relative = Path(reference)
        if relative.is_absolute() or ".." in relative.parts:
            raise ContractError(f"{path}: style reference must stay inside the repository: {reference!r}")
        target = (resolved_repo / relative).resolve()
        try:
            target.relative_to(resolved_repo)
        except ValueError as exc:
            raise ContractError(f"{path}: style reference escapes the repository: {reference!r}") from exc
        if not target.is_file():
            raise ContractError(f"{path}: missing style reference {reference!r}")

    playback = data.get("actionPlayback") if schema_version == 2 else {
        action: {
            "mode": "cycle", "variantCount": 1, "framesPerVariant": 8,
            "framesPerSecond": 8, "loop": True,
        }
        for action in actions
    }
    plans = data.get("framePlans") if schema_version == 2 else data.get("actionCycles")
    if not isinstance(playback, dict) or set(playback) != set(actions):
        raise ContractError(f"{path}: actionPlayback must describe every declared action exactly once")
    if not isinstance(plans, dict):
        raise ContractError(f"{path}: {'framePlans' if schema_version == 2 else 'actionCycles'} must be an object")
    for action in actions:
        config = playback[action]
        if not isinstance(config, dict):
            raise ContractError(f"{path}: actionPlayback.{action} must be an object")
        mode = config.get("mode")
        shape = (config.get("variantCount"), config.get("framesPerVariant"), config.get("loop"))
        expected_shape = {
            "cycle": (1, 8, True),
            "variant-loop": (4, 2, True),
            "static-variants": (8, 1, False),
        }.get(mode)
        if expected_shape is None or shape != expected_shape:
            raise ContractError(f"{path}: invalid actionPlayback for {action!r}")
        fps = config.get("framesPerSecond")
        if not isinstance(fps, int) or fps < 0 or (mode == "static-variants") != (fps == 0):
            raise ContractError(f"{path}: invalid framesPerSecond for {action!r}")
        if mode == "variant-loop" and action != "idle":
            raise ContractError(f"{path}: only idle may use variant-loop")
        if mode == "static-variants" and action != "dead":
            raise ContractError(f"{path}: only dead may use static-variants")
        plan = plans.get(action)
        if not isinstance(plan, list) or len(plan) != FRAME_COUNT:
            raise ContractError(f"{path}: {action} needs exactly 8 frame plans")
        poses: list[str] = []
        for frame, entry in enumerate(plan):
            if not isinstance(entry, dict):
                raise ContractError(f"{path}: {action} frame {frame} must be an object")
            if entry.get("frame") != frame:
                raise ContractError(f"{path}: {action} entry {frame} must declare frame={frame}")
            if mode == "cycle" and entry.get("phase") != f"{frame}/8":
                raise ContractError(f"{path}: {action} frame {frame} must declare phase={frame}/8")
            if mode == "variant-loop" and (
                entry.get("variant") != frame // 2 or entry.get("phase") != f"{frame % 2}/2"
            ):
                raise ContractError(f"{path}: idle frame {frame} must map to variant={frame // 2}, phase={frame % 2}/2")
            if mode == "static-variants" and (
                entry.get("variant") != frame or entry.get("phase") != "static"
            ):
                raise ContractError(f"{path}: dead frame {frame} must map to static variant={frame}")
            pose = entry.get("pose")
            if not isinstance(pose, str) or not pose.strip():
                raise ContractError(f"{path}: describe {action} frame {frame}")
            poses.append(pose.strip().casefold())
        if len(poses) != len(set(poses)):
            raise ContractError(f"{path}: {action} frame descriptions must be unique")
    return data


def paeth(a: int, b: int, c: int) -> int:
    p = a + b - c
    distances = (abs(p - a), abs(p - b), abs(p - c))
    return (a, b, c)[distances.index(min(distances))]


def read_png(path: Path) -> tuple[int, int, bytes]:
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise ContractError(f"cannot read {path}: {exc}") from exc
    if not raw.startswith(PNG_SIG):
        raise ContractError(f"{path}: not a PNG")
    pos, ihdr, idat = len(PNG_SIG), None, []
    while pos + 12 <= len(raw):
        length = struct.unpack(">I", raw[pos:pos + 4])[0]
        kind = raw[pos + 4:pos + 8]
        end = pos + 8 + length
        payload = raw[pos + 8:end]
        if end + 4 > len(raw):
            raise ContractError(f"{path}: truncated PNG chunk")
        expected = struct.unpack(">I", raw[end:end + 4])[0]
        if (binascii.crc32(kind + payload) & 0xFFFFFFFF) != expected:
            raise ContractError(f"{path}: corrupt PNG chunk")
        if kind == b"IHDR":
            ihdr = struct.unpack(">IIBBBBB", payload)
        elif kind == b"IDAT":
            idat.append(payload)
        elif kind == b"IEND":
            break
        pos = end + 4
    if ihdr is None:
        raise ContractError(f"{path}: missing IHDR")
    width, height, depth, color, compression, filtering, interlace = ihdr
    if (depth, color, compression, filtering, interlace) != (8, 6, 0, 0, 0):
        raise ContractError(f"{path}: require non-interlaced 8-bit RGBA PNG")
    try:
        scans = zlib.decompress(b"".join(idat))
    except zlib.error as exc:
        raise ContractError(f"{path}: invalid compressed pixels: {exc}") from exc
    stride = width * 4
    if len(scans) != height * (stride + 1):
        raise ContractError(f"{path}: unexpected scanline length")
    output, prior, src = bytearray(height * stride), bytearray(stride), 0
    for y in range(height):
        mode, src = scans[src], src + 1
        encoded, src = scans[src:src + stride], src + stride
        row = bytearray(stride)
        for x, byte in enumerate(encoded):
            left = row[x - 4] if x >= 4 else 0
            up = prior[x]
            diagonal = prior[x - 4] if x >= 4 else 0
            predictors = (0, left, up, (left + up) // 2, paeth(left, up, diagonal))
            if mode > 4:
                raise ContractError(f"{path}: invalid PNG filter {mode}")
            row[x] = (byte + predictors[mode]) & 0xFF
        output[y * stride:(y + 1) * stride] = row
        prior = row
    return width, height, bytes(output)


def chunk(kind: bytes, payload: bytes) -> bytes:
    crc = binascii.crc32(kind + payload) & 0xFFFFFFFF
    return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", crc)


def write_png(path: Path, width: int, height: int, pixels: bytes) -> None:
    stride = width * 4
    scans = b"".join(b"\0" + pixels[y * stride:(y + 1) * stride] for y in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    raw = (
        PNG_SIG
        + chunk(b"IHDR", ihdr)
        + chunk(b"sRGB", b"\0")
        + chunk(b"IDAT", zlib.compress(scans, 9))
        + chunk(b"IEND", b"")
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(raw)


def validate_frame(path: Path) -> bytes:
    if not path.is_file():
        raise ContractError(f"missing {path}")
    width, height, pixels = read_png(path)
    if (width, height) != (CELL_W, CELL_H):
        raise ContractError(f"{path}: got {width}x{height}; require 200x300")
    if sum(alpha > 0 for alpha in pixels[3::4]) < (CELL_W * CELL_H) // 100:
        raise ContractError(f"{path}: sprite is empty or too small")
    for y in range(CELL_H):
        for x in range(CELL_W):
            offset = (y * CELL_W + x) * 4
            alpha = pixels[offset + 3]
            if (x < 4 or x >= 196 or y < 4 or y >= 280) and alpha:
                raise ContractError(f"{path}: pixel outside safety bounds at ({x},{y})")
            if alpha == 0 and pixels[offset:offset + 3] != b"\0\0\0":
                raise ContractError(f"{path}: transparent RGB not cleared at ({x},{y})")
    return pixels


def actions_for(data: dict, action: str | None) -> list[str]:
    if action is None:
        return list(data["actions"])
    check_id(action, "action")
    if action not in data["actions"]:
        raise ContractError(f"action {action!r} is not declared in role.json")
    return [action]


def validate_references(repo: Path, set_id: str, role: str) -> None:
    base = source_root(repo, set_id, role) / "references"
    validate_frame(base / "front.png")
    validate_frame(base / "back.png")


def validate_action(repo: Path, set_id: str, role: str, action: str) -> None:
    base = source_root(repo, set_id, role) / "frames" / action
    expected = {f"frame-{frame}.png" for frame in range(FRAME_COUNT)}
    for direction in DIRECTIONS:
        directory = base / direction
        if not directory.is_dir():
            raise ContractError(f"missing {directory}")
        actual = {path.name for path in directory.glob("*.png")}
        if actual != expected:
            raise ContractError(
                f"{directory}: frame mismatch; missing={sorted(expected - actual)}, extra={sorted(actual - expected)}"
            )
        seen: dict[str, int] = {}
        for frame in range(FRAME_COUNT):
            pixels = validate_frame(directory / f"frame-{frame}.png")
            digest = hashlib.sha256(pixels).hexdigest()
            if digest in seen:
                raise ContractError(f"{directory}: exact duplicate frames {seen[digest]} and {frame}")
            seen[digest] = frame


def load_research(repo: Path, set_id: str, role: str) -> dict:
    path = source_root(repo, set_id, role) / "research.json"
    if not path.is_file():
        raise ContractError(f"missing {path}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ContractError(f"cannot read {path}: {exc}") from exc
    if data.get("schemaVersion") != 2 or data.get("role") != role:
        raise ContractError(f"{path}: require schemaVersion 2 and role {role!r}")
    if data.get("status") != "ready":
        raise ContractError(f"{path}: set status to 'ready' after completing and checking the research")
    sources = data.get("sources")
    if not isinstance(sources, list) or len(sources) < 2:
        raise ContractError(f"{path}: cite at least two independent sources")
    source_ids: set[str] = set()
    source_hosts: set[str] = set()
    for source in sources:
        if not isinstance(source, dict):
            raise ContractError(f"{path}: every source must be an object")
        source_id = source.get("id")
        if not isinstance(source_id, str):
            raise ContractError(f"{path}: every source needs a string id")
        check_id(source_id, "source")
        parsed = urlparse(source.get("url", ""))
        if parsed.scheme != "https" or not parsed.netloc:
            raise ContractError(f"{path}: source {source_id!r} needs a public HTTPS URL")
        for key in ("title", "publisher", "accessedAt"):
            if not isinstance(source.get(key), str) or not source[key].strip():
                raise ContractError(f"{path}: source {source_id!r} needs {key}")
        try:
            dt.date.fromisoformat(source["accessedAt"])
        except ValueError as exc:
            raise ContractError(f"{path}: source {source_id!r} accessedAt must be YYYY-MM-DD") from exc
        if source_id in source_ids:
            raise ContractError(f"{path}: duplicate source id {source_id!r}")
        source_ids.add(source_id)
        source_hosts.add(parsed.netloc.casefold())
    if len(source_hosts) < 2:
        raise ContractError(f"{path}: use at least two independent source domains")

    claims = data.get("claims")
    if not isinstance(claims, list):
        raise ContractError(f"{path}: claims must be an array")
    covered: set[str] = set()
    for claim in claims:
        if not isinstance(claim, dict):
            raise ContractError(f"{path}: every claim must be an object")
        category, statement, citations = claim.get("category"), claim.get("statement"), claim.get("sourceIds")
        if category not in RESEARCH_CATEGORIES:
            raise ContractError(f"{path}: invalid claim category {category!r}")
        if not isinstance(statement, str) or not statement.strip():
            raise ContractError(f"{path}: every claim needs a statement")
        if not isinstance(citations, list) or not citations or any(item not in source_ids for item in citations):
            raise ContractError(f"{path}: every claim needs valid sourceIds")
        if claim.get("confidence") not in ("high", "medium", "low"):
            raise ContractError(f"{path}: every claim confidence must be high, medium, or low")
        covered.add(category)
    missing_categories = set(RESEARCH_CATEGORIES) - covered
    if missing_categories:
        raise ContractError(f"{path}: missing researched claim categories {sorted(missing_categories)}")

    visual = data.get("visualDefinition")
    if not isinstance(visual, dict):
        raise ContractError(f"{path}: visualDefinition must be an object")
    fields = ("silhouette", "garments", "armor", "weapons", "handedness", "palette")
    missing = [field for field in fields if not isinstance(visual.get(field), str) or not visual[field].strip()]
    invariants = visual.get("invariants")
    if missing or not isinstance(invariants, list) or not invariants or any(
        not isinstance(item, str) or not item.strip() for item in invariants
    ):
        raise ContractError(f"{path}: complete visualDefinition fields and at least one invariant; missing={missing}")

    if data.get("actionPlanStatus") != "ready":
        raise ContractError(f"{path}: set actionPlanStatus to 'ready' after role-specific motion review")
    role_path = source_root(repo, set_id, role) / "role.json"
    try:
        role_data = json.loads(role_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ContractError(f"cannot read {role_path}: {exc}") from exc
    actions, playback, plans = role_data.get("actions"), role_data.get("actionPlayback"), data.get("framePlans")
    if not isinstance(actions, list) or not isinstance(playback, dict) or not isinstance(plans, dict):
        raise ContractError(f"{path}: cannot validate researched framePlans against role.json")
    if set(plans) != set(actions):
        raise ContractError(f"{path}: framePlans must describe every role action exactly once")
    for action in actions:
        config, plan = playback.get(action), plans.get(action)
        if not isinstance(config, dict) or not isinstance(plan, list) or len(plan) != FRAME_COUNT:
            raise ContractError(f"{path}: {action} needs exactly 8 researched frame plans")
        mode = config.get("mode")
        poses: list[str] = []
        for frame, entry in enumerate(plan):
            if not isinstance(entry, dict) or entry.get("frame") != frame:
                raise ContractError(f"{path}: {action} frame {frame} metadata is invalid")
            if mode == "cycle" and entry.get("phase") != f"{frame}/8":
                raise ContractError(f"{path}: {action} frame {frame} must declare phase={frame}/8")
            if mode == "variant-loop" and (
                entry.get("variant") != frame // 2 or entry.get("phase") != f"{frame % 2}/2"
            ):
                raise ContractError(f"{path}: idle frame {frame} has invalid variant/phase")
            if mode == "static-variants" and (
                entry.get("variant") != frame or entry.get("phase") != "static"
            ):
                raise ContractError(f"{path}: dead frame {frame} has invalid static variant")
            pose = entry.get("pose")
            if not isinstance(pose, str) or not pose.strip():
                raise ContractError(f"{path}: describe role-specific {action} frame {frame}")
            poses.append(pose.strip().casefold())
        if len(poses) != len(set(poses)):
            raise ContractError(f"{path}: {action} researched frame descriptions must be unique")
    return data


def cmd_import_simulator(args: argparse.Namespace) -> None:
    repo = args.repo.resolve()
    profile = load_simulation_profile(repo, args.profile)
    context = profile["historical_context"]
    defaults = profile["asset_defaults"]
    style_id = args.style_id or defaults["style_id"]
    check_id(style_id, "style")
    if style_id == DEFAULT_RENDER_STYLE["preset"]:
        style = json.loads(json.dumps(DEFAULT_RENDER_STYLE))
        if args.style_prompt:
            style["prompt"] = args.style_prompt
    else:
        style_prompt = args.style_prompt or defaults.get("style_prompt")
        if not isinstance(style_prompt, str) or not style_prompt.strip():
            raise ContractError("custom style needs --style-prompt or asset_defaults.style_prompt")
        style = {
            "preset": style_id,
            "prompt": style_prompt,
            "avoid": [item for item in (args.style_avoid or "").split(",") if item],
            "referenceImages": [],
        }
    set_id = args.set or f"{context['region_id']}-{context['period_id']}-{style_id}"
    expected_set = f"{context['region_id']}-{context['period_id']}-{style_id}"
    if set_id != expected_set:
        raise ContractError(f"--set must equal simulator context and style: {expected_set}")
    check_id(set_id, "asset set")
    role_ids = [role["id"] for role in profile["roles"]]
    action_definitions = {action["id"]: action for action in profile["actions"]}
    set_path = set_root(repo, set_id) / "set.json"
    set_payload = {
        "schemaVersion": 1,
        "id": set_id,
        "period": {"id": context["period_id"], "label": context["period_label"]},
        "region": {"id": context["region_id"], "label": context["region_label"]},
        "styleId": style_id,
        "renderStyle": style,
        "simulationProfile": profile["id"],
        "roles": role_ids,
    }
    if set_path.exists():
        existing = load_set(repo, set_id)
        comparable = {key: existing.get(key) for key in set_payload}
        if comparable != set_payload:
            raise ContractError(f"{set_path}: existing asset set conflicts with simulator profile")
    else:
        write_json(set_path, set_payload)

    created: list[Path] = []
    for role in profile["roles"]:
        root = source_root(repo, set_id, role["id"])
        role_path = root / "role.json"
        if role_path.exists():
            current = json.loads(role_path.read_text(encoding="utf-8"))
            simulation = current.get("simulation", {})
            expected_binding = {
                "profile": profile["id"],
                "troopTypeId": role["troop_type_id"],
                "troopTypeIndex": role["troop_type_index"],
                "description": role["description"],
                "stateActions": role["state_actions"],
            }
            expected_playback = {
                action: {
                    "mode": action_definitions[action]["playback"],
                    "variantCount": action_definitions[action]["variant_count"],
                    "framesPerVariant": action_definitions[action]["frames_per_variant"],
                    "framesPerSecond": action_definitions[action]["frames_per_second"],
                    "loop": action_definitions[action]["loop"],
                }
                for action in role["actions"]
            }
            definition_changed = (
                simulation != expected_binding
                or current.get("actions") != role["actions"]
                or current.get("actionPlayback") != expected_playback
            )
            if definition_changed and any(root.glob("frames/**/*.png")):
                raise ContractError(
                    f"{role_path}: simulator definition changed after frames were generated; "
                    "create a new asset set or explicitly migrate the approved artwork"
                )
            if definition_changed:
                current["schemaVersion"] = 2
                current["simulation"] = expected_binding
                current["actions"] = role["actions"]
                current["actionPlayback"] = expected_playback
                current.pop("actionCycles", None)
                current["framePlans"] = {
                    action: frame_plan(action_definitions[action]) for action in role["actions"]
                }
                write_json(role_path, current)
                created.append(role_path)
        else:
            payload = {
                "schemaVersion": 2,
                "role": role["id"],
                "assetSet": {
                    "id": set_id,
                    "periodId": context["period_id"],
                    "regionId": context["region_id"],
                    "styleId": style_id,
                },
                "displayName": role["name_en"],
                "displayNameJa": role["name_ja"],
                "simulation": {
                    "profile": profile["id"],
                    "troopTypeId": role["troop_type_id"],
                    "troopTypeIndex": role["troop_type_index"],
                    "description": role["description"],
                    "stateActions": role["state_actions"],
                },
                "actions": role["actions"],
                "actionPlayback": {
                    action: {
                        "mode": action_definitions[action]["playback"],
                        "variantCount": action_definitions[action]["variant_count"],
                        "framesPerVariant": action_definitions[action]["frames_per_variant"],
                        "framesPerSecond": action_definitions[action]["frames_per_second"],
                        "loop": action_definitions[action]["loop"],
                    }
                    for action in role["actions"]
                },
                "identity": {
                    "period": context["period_label"],
                    "region": context["region_label"],
                    "silhouette": "",
                    "garments": "",
                    "armor": "",
                    "weapons": "",
                    "handedness": "",
                    "palette": "",
                    "invariants": [],
                },
                "renderStyle": style,
                "research": {"status": "needs-research", "sourceIds": []},
                "framePlans": {
                    action: frame_plan(action_definitions[action]) for action in role["actions"]
                },
            }
            write_json(role_path, payload)
            created.append(role_path)
        research_path = root / "research.json"
        if not research_path.exists():
            write_json(research_path, role_research_template(profile, role))
            created.append(research_path)
        else:
            existing_research = json.loads(research_path.read_text(encoding="utf-8"))
            if existing_research.get("status") == "needs-research" and (
                existing_research.get("schemaVersion") != 2
                or existing_research.get("brief", {}).get("actions") != role["actions"]
            ):
                write_json(research_path, role_research_template(profile, role))
                created.append(research_path)
        (root / "references").mkdir(parents=True, exist_ok=True)
        for action in role["actions"]:
            for direction in DIRECTIONS:
                (root / "frames" / action / direction).mkdir(parents=True, exist_ok=True)

    runtime = {
        "schemaVersion": 1,
        "id": profile["id"],
        "assetSetId": set_id,
        "historicalContext": {
            "period": {"id": context["period_id"], "label": context["period_label"]},
            "region": {"id": context["region_id"], "label": context["region_label"]},
        },
        "styleId": style_id,
        "actionPlayback": {
            action["id"]: {
                "mode": action["playback"],
                "variantCount": action["variant_count"],
                "framesPerVariant": action["frames_per_variant"],
                "framesPerSecond": action["frames_per_second"],
                "loop": action["loop"],
            }
            for action in profile["actions"]
        },
        "roles": [
            {
                "id": role["id"],
                "troopTypeId": role["troop_type_id"],
                "troopTypeIndex": role["troop_type_index"],
                "name": role["name_en"],
                "description": role["description"],
                "actions": role["actions"],
                "stateActions": role["state_actions"],
            }
            for role in profile["roles"]
        ],
    }
    runtime_path = repo / "web/public/assets/sprites/v2/profiles" / f"{profile['id']}.json"
    write_json(runtime_path, runtime)
    print(f"imported simulator profile {profile['id']!r} into asset set {set_id!r}")
    print(f"runtime profile: {runtime_path}")
    for path in created:
        print(f"created: {path}")


def cmd_apply_research(args: argparse.Namespace) -> None:
    repo = args.repo.resolve()
    check_id(args.set, "asset set")
    check_id(args.role, "role")
    research = load_research(repo, args.set, args.role)
    role_path = source_root(repo, args.set, args.role) / "role.json"
    try:
        role_data = json.loads(role_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ContractError(f"cannot read {role_path}: {exc}") from exc
    visual = research["visualDefinition"]
    role_data["identity"].update({key: visual[key] for key in (
        "silhouette", "garments", "armor", "weapons", "handedness", "palette", "invariants"
    )})
    role_data["research"] = {
        "status": "ready",
        "sourceIds": [source["id"] for source in research["sources"]],
        "sourceFile": "research.json",
    }
    role_data["framePlans"] = research["framePlans"]
    write_json(role_path, role_data)
    load_role(repo, args.set, args.role)
    print(f"applied researched visual definition: {role_path}")


def cmd_init(args: argparse.Namespace) -> None:
    repo, role, set_id = args.repo.resolve(), args.role, args.set
    check_id(role, "role")
    check_id(set_id, "asset set")
    for value, kind in (
        (args.period_id, "period"), (args.region_id, "region"), (args.style_id, "style")
    ):
        check_id(value, kind)
    expected_set = f"{args.region_id}-{args.period_id}-{args.style_id}"
    if set_id != expected_set:
        raise ContractError(f"--set must equal <region-id>-<period-id>-<style-id>: {expected_set}")
    actions = args.actions.split(",")
    if not actions or any(not action for action in actions) or len(actions) != len(set(actions)):
        raise ContractError("--actions must contain unique comma-separated IDs")
    for action in actions:
        check_id(action, "action")
    root = source_root(repo, set_id, role)
    role_json = root / "role.json"
    if role_json.exists():
        raise ContractError(f"refusing to overwrite {role_json}")
    style = json.loads(json.dumps(DEFAULT_RENDER_STYLE))
    if args.style_id != DEFAULT_RENDER_STYLE["preset"]:
        if not args.style_prompt:
            raise ContractError("custom --style-id requires --style-prompt")
        style = {
            "preset": args.style_id,
            "prompt": args.style_prompt,
            "avoid": [item for item in args.style_avoid.split(",") if item] if args.style_avoid else [],
            "referenceImages": [],
        }
    elif args.style_prompt:
        style["prompt"] = args.style_prompt
    set_path = set_root(repo, set_id) / "set.json"
    if set_path.exists():
        set_data = load_set(repo, set_id)
        expected_metadata = (
            set_data["period"]["id"], set_data["period"]["label"],
            set_data["region"]["id"], set_data["region"]["label"],
            set_data["styleId"], set_data["renderStyle"],
        )
        requested_metadata = (
            args.period_id, args.period, args.region_id, args.region, args.style_id, style,
        )
        if expected_metadata != requested_metadata:
            raise ContractError(f"{set_path}: requested metadata conflicts with the existing asset set")
        if role not in set_data["roles"]:
            set_data["roles"].append(role)
            set_data["roles"].sort()
    else:
        set_data = {
            "schemaVersion": 1,
            "id": set_id,
            "period": {"id": args.period_id, "label": args.period},
            "region": {"id": args.region_id, "label": args.region},
            "styleId": args.style_id,
            "renderStyle": style,
            "roles": [role],
        }
    write_json(set_path, set_data)
    root.mkdir(parents=True, exist_ok=True)
    manual_action_definitions = {
        action: (
            {
                "id": action, "playback": "variant-loop", "variant_count": 4,
                "frames_per_variant": 2, "frames_per_second": 2, "loop": True,
            }
            if action == "idle"
            else {
                "id": action, "playback": "static-variants", "variant_count": 8,
                "frames_per_variant": 1, "frames_per_second": 0, "loop": False,
            }
            if action == "dead"
            else {
                "id": action, "playback": "cycle", "variant_count": 1,
                "frames_per_variant": 8, "frames_per_second": 8, "loop": True,
            }
        )
        for action in actions
    }
    payload = {
        "schemaVersion": 2,
        "role": role,
        "assetSet": {
            "id": set_id,
            "periodId": args.period_id,
            "regionId": args.region_id,
            "styleId": args.style_id,
        },
        "displayName": role.replace("-", " ").title(),
        "actions": actions,
        "actionPlayback": {
            action: {
                "mode": definition["playback"],
                "variantCount": definition["variant_count"],
                "framesPerVariant": definition["frames_per_variant"],
                "framesPerSecond": definition["frames_per_second"],
                "loop": definition["loop"],
            }
            for action, definition in manual_action_definitions.items()
        },
        "identity": {
            "period": args.period, "region": args.region,
            "silhouette": "", "garments": "", "armor": "",
            "weapons": "", "handedness": "", "palette": "", "invariants": [],
        },
        "renderStyle": style,
        "framePlans": {
            action: frame_plan(manual_action_definitions[action])
            if action in ACTION_CYCLE_POSES or action in ("idle", "dead") else [
                {"frame": i, "phase": f"{i}/8", "pose": ""} for i in range(8)
            ]
            for action in actions
        },
    }
    write_json(role_json, payload)
    (root / "references").mkdir(exist_ok=True)
    for action in actions:
        for direction in DIRECTIONS:
            (root / "frames" / action / direction).mkdir(parents=True, exist_ok=True)
    print(f"initialized role {role!r} in asset set {set_id!r}: {root}")


def cmd_validate(args: argparse.Namespace) -> None:
    repo = args.repo.resolve()
    check_id(args.role, "role")
    check_id(args.set, "asset set")
    data = load_role(repo, args.set, args.role)
    if args.definition_only:
        print(f"valid definition: {args.role}")
        return
    validate_references(repo, args.set, args.role)
    selected = [] if args.references_only else actions_for(data, args.action)
    for action in selected:
        validate_action(repo, args.set, args.role, action)
    scope = "references and phase plans" if args.references_only else f"actions={','.join(selected)}"
    print(f"valid: {args.role} {scope}")


def assemble_one(repo: Path, set_id: str, role: str, action: str) -> Path:
    validate_action(repo, set_id, role, action)
    sheet_w, sheet_h = CELL_W * 8, CELL_H * 8
    sheet = bytearray(sheet_w * sheet_h * 4)
    base = source_root(repo, set_id, role) / "frames" / action
    for frame in range(8):
        for column, direction in enumerate(DIRECTIONS):
            pixels = validate_frame(base / direction / f"frame-{frame}.png")
            for y in range(CELL_H):
                src = y * CELL_W * 4
                dst = (((frame * CELL_H + y) * sheet_w) + column * CELL_W) * 4
                sheet[dst:dst + CELL_W * 4] = pixels[src:src + CELL_W * 4]
    output = repo / "web/public/assets/sprites/v2/sets" / set_id / role / f"{action}.png"
    write_png(output, sheet_w, sheet_h, bytes(sheet))
    return output


def update_manifest(
    repo: Path, set_id: str, role: str, outputs: dict[str, Path], set_data: dict, role_data: dict
) -> Path:
    path = repo / "web/public/assets/sprites/v2/manifest.json"
    if path.exists():
        try:
            manifest = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ContractError(f"cannot read {path}: {exc}") from exc
    else:
        manifest = {
            "version": 2,
            "cell": {"width": 200, "height": 300},
            "columns": {"kind": "direction", "values": list(DIRECTIONS)},
            "rows": {"kind": "frame", "values": list(range(8))},
            "rowSemantics": "per-role-actionPlayback",
            "sets": {},
        }
    contract = (2, {"width": 200, "height": 300}, list(DIRECTIONS), list(range(8)))
    found = (
        manifest.get("version"), manifest.get("cell"),
        manifest.get("columns", {}).get("values"), manifest.get("rows", {}).get("values"),
    )
    if found != contract:
        raise ContractError(f"{path}: manifest conflicts with v2 contract")
    if "roles" in manifest or not isinstance(manifest.setdefault("sets", {}), dict):
        raise ContractError(f"{path}: require set-scoped v2 manifest schema")
    set_metadata = {
        "period": set_data["period"],
        "region": set_data["region"],
        "styleId": set_data["styleId"],
    }
    if set_data.get("simulationProfile") is not None:
        set_metadata["simulationProfile"] = set_data["simulationProfile"]
    sets = manifest["sets"]
    if set_id in sets:
        existing = {key: sets[set_id].get(key) for key in set_metadata}
        if existing != set_metadata:
            raise ContractError(f"{path}: conflicting metadata for asset set {set_id!r}")
    else:
        sets[set_id] = {**set_metadata, "declaredRoles": [], "roles": {}}
    set_entry = sets[set_id]
    set_entry["declaredRoles"] = list(set_data["roles"])
    role_entry = set_entry.setdefault("roles", {}).setdefault(role, {"actions": {}})
    if role_data.get("simulation") is not None:
        role_entry["simulation"] = role_data["simulation"]
    role_entry["actionPlayback"] = role_data.get("actionPlayback", {
        action: {
            "mode": "cycle", "variantCount": 1, "framesPerVariant": 8,
            "framesPerSecond": 8, "loop": True,
        }
        for action in role_data["actions"]
    })
    actions = role_entry.setdefault("actions", {})
    assets = repo / "web/public/assets"
    for action, output in outputs.items():
        if not output.is_file():
            raise ContractError(f"cannot map missing output {output}")
        relative = output.relative_to(assets)
        expected = Path("sprites/v2/sets") / set_id / role / f"{action}.png"
        if relative != expected:
            raise ContractError(f"output path {relative} must equal canonical path {expected}")
        actions[action] = relative.as_posix()
    mapped: dict[str, str] = {}
    for mapped_set, set_value in sets.items():
        for mapped_role, role_value in set_value.get("roles", {}).items():
            for mapped_action, target in role_value.get("actions", {}).items():
                key = f"{mapped_set}/{mapped_role}/{mapped_action}"
                if not isinstance(target, str):
                    raise ContractError(f"{path}: target for {key} must be a string")
                target_path = Path(target)
                expected_target = Path("sprites/v2/sets") / mapped_set / mapped_role / f"{mapped_action}.png"
                if target_path.is_absolute() or ".." in target_path.parts or target_path != expected_target:
                    raise ContractError(f"{path}: noncanonical target {target!r} for {key}")
                if not (assets / target_path).is_file():
                    raise ContractError(f"{path}: missing mapped target {target!r}")
                if target in mapped:
                    raise ContractError(f"{path}: duplicate target {target!r} for {mapped[target]} and {key}")
                mapped[target] = key
    write_json(path, manifest)
    return path


def cmd_assemble(args: argparse.Namespace) -> None:
    repo = args.repo.resolve()
    check_id(args.role, "role")
    check_id(args.set, "asset set")
    set_data = load_set(repo, args.set)
    data = load_role(repo, args.set, args.role)
    validate_references(repo, args.set, args.role)
    outputs = {
        action: assemble_one(repo, args.set, args.role, action)
        for action in actions_for(data, args.action)
    }
    manifest = update_manifest(repo, args.set, args.role, outputs, set_data, data)
    for output in outputs.values():
        print(f"assembled: {output}")
    print(f"updated: {manifest}")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    sub = root.add_subparsers(dest="command", required=True)
    init = sub.add_parser("init")
    init.add_argument("--repo", type=Path, required=True)
    init.add_argument("--set", default=DEFAULT_SET_ID)
    init.add_argument("--role", required=True)
    init.add_argument("--actions", required=True)
    init.add_argument("--period-id", default=DEFAULT_PERIOD_ID)
    init.add_argument("--period", default="14th century CE")
    init.add_argument("--region-id", default=DEFAULT_REGION_ID)
    init.add_argument("--region", default="Europe")
    init.add_argument("--style-id", default=DEFAULT_RENDER_STYLE["preset"])
    init.add_argument("--style-prompt")
    init.add_argument("--style-avoid", help="comma-separated custom-style avoid terms")
    init.set_defaults(run=cmd_init)
    import_sim = sub.add_parser("import-simulator")
    import_sim.add_argument("--repo", type=Path, required=True)
    import_sim.add_argument("--profile", default="medieval_western")
    import_sim.add_argument("--set")
    import_sim.add_argument("--style-id")
    import_sim.add_argument("--style-prompt")
    import_sim.add_argument("--style-avoid", help="comma-separated custom-style avoid terms")
    import_sim.set_defaults(run=cmd_import_simulator)
    apply_research = sub.add_parser("apply-research")
    apply_research.add_argument("--repo", type=Path, required=True)
    apply_research.add_argument("--set", default=DEFAULT_SET_ID)
    apply_research.add_argument("--role", required=True)
    apply_research.set_defaults(run=cmd_apply_research)
    validate = sub.add_parser("validate")
    validate.add_argument("--repo", type=Path, required=True)
    validate.add_argument("--set", default=DEFAULT_SET_ID)
    validate.add_argument("--role", required=True)
    validate.add_argument("--action")
    validate.add_argument("--references-only", action="store_true")
    validate.add_argument("--definition-only", action="store_true")
    validate.set_defaults(run=cmd_validate)
    assemble = sub.add_parser("assemble")
    assemble.add_argument("--repo", type=Path, required=True)
    assemble.add_argument("--set", default=DEFAULT_SET_ID)
    assemble.add_argument("--role", required=True)
    assemble.add_argument("--action")
    assemble.set_defaults(run=cmd_assemble)
    return root


def main() -> int:
    args = parser().parse_args()
    try:
        args.run(args)
    except ContractError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
