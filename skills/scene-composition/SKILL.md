---
name: scene-composition
description: Author valid SceneSpec YAML or JSON directly without invoking SceneComposer. Use when creating or editing a SceneSpec, staging foreground slots, assigning ExclusiveSurfaceMap colors, enforcing positive-only phrasing, matching Visibility, or deciding when to pair a Blockout SpatialGuide.
---

# Scene Composition (`SceneSpec` Direct Authoring)

Write `SceneSpec` (`schema_version: 1`) directly in YAML or JSON so `PromptCompiler` validates and compiles it in sub-millisecond Rust without double-LLM drift or `SceneComposer` latency.

`SceneSpec` deserializes with `deny_unknown_fields`: include only the schema fields defined in Section 2, then validate before generation.

## 1. Six Composition Rules

1. **Single continuous camera shot and metric `depth_bands`:**
   - Specify one unified camera view via `camera.shot` (plus optional `angle` and `framing`) covering a single continuous scene.
   - Partition the scene with at least one `depth_bands` entry (`plane`: `foreground`, `midground`, or `background`) using finite metric distances where `near_meters >= 0.0` and `far_meters > near_meters`.
2. **Attention budget (`<= 3` left-to-right `foreground_slots`):**
   - Include at most `3` entries in `foreground_slots`.
   - Order `foreground_slots` strictly left-to-right with finite `position_x_ratio` values in `[0.0, 1.0]` that strictly increase from slot to slot (`slot[i].position_x_ratio > slot[i - 1].position_x_ratio`).
3. **Metric scale, explicit hand bindings, and positive prop geometry:**
   - Set finite positive `height_meters` (`> 0.0`) on every foreground slot so relative scale stays physically grounded across characters, creatures, and props.
   - Bind both `left_hand` and `right_hand` explicitly on every slot (`item: string | null` and `action: string`, e.g., `item: null`, `action: "open palm resting on leather belt"`).
   - Describe held weapons, tools, and silhouettes with concrete positive structural geometry (segments, proportions, material joins) aligned with the `Subject`'s `positive_weapon_geometry`.
4. **Exhaustive `ExclusiveSurfaceMap` separated from `Atmosphere`:**
   - Populate `exclusive_surface_map.bindings` on every slot with at least one unique `slot` name, physical `material`, exact 6-digit `#RRGGBB` `color_hex`, and optional `finish`.
   - Record unshaded local base albedo colors matching the `Subject`'s `PaletteLock` inside `exclusive_surface_map`; keep directional lighting, `color_temperature_k`, `max_ambient_tint_blend`, `exposure_floor`, and weather inside the referenced `Atmosphere` (`atmosphere_ref`), and rendering medium/linework inside `style_ref`.
5. **Positive-only visual phrasing:**
   - State only what is visibly present in `title`, `action_summary`, `camera.*`, `depth_bands[].description`, `pose`, `left_hand`/`right_hand`, and `exclusive_surface_map` (`material`, `finish`).
   - `PromptCompiler` rejects active negation tokens (`no`, `not`, `never`, `without`, `avoid`, `avoiding`, `exclude`, `excluding`, `except`, `neither`, `nor`, `none`, `nothing`, `absent`, `lacking`, `lacks`, `missing`, `minus`, contractions like `don't`/`isn't`) and negation phrases (`free of`, `devoid of`, `absence of`, `do not`, `instead of`, etc.) fail-closed because image models frequently render negated nouns.
   - Replace absence phrases with positive visual descriptions (for example, write `"bare stone deck"` instead of `"deck free of barrels"`, and `"windswept uncovered hair"` instead of `"without a helmet"`).
6. **`Visibility` matching and `Blockout` `SpatialGuide` pairing:**
   - Set top-level `visibility` (`public`, `restricted`, or `internal`) and ensure every referenced `Subject` (`slot.visibility` and resolved `Subject` `card.yaml` `visibility`) satisfies `subject.visibility <= scene.visibility`.
   - Pair the `SceneSpec` with a `Blockout` `GuideSpec` (`guide` tool / `imgforge guide render`) whenever a scene stages `>= 4` total actors across depth planes, tight action trajectories, overlapping vertical elevations, or precise relative heights that need spatial anchoring beyond left-to-right `position_x_ratio`.

## 2. Canonical `SceneSpec` Schema (`schema_version: 1`)

All prose fields must be non-empty and free of non-whitespace control characters; identifier fields (`id`, `style_ref`, `atmosphere_ref`, `subject_ref`, `bindings[].slot`) must be non-empty without control characters (use lowercase kebab-case slugs for refs). `width` and `height` are optional positive integers (default `1024x1024`, bounded by `1..=16384` per axis and `<= 67,108,864` pixels).

```yaml
schema_version: 1
id: harbor-lantern-exchange
title: Northern Pier Lantern Exchange
visibility: public
camera:
  shot: medium-wide 35mm
  angle: eye-level three-quarter
  framing: horizontal rule-of-thirds
depth_bands:
  - plane: foreground
    near_meters: 1.5
    far_meters: 4.5
    description: damp timber pier planks and iron mooring bollard
  - plane: background
    near_meters: 18.0
    far_meters: 90.0
    description: slate headland and distant stone beacon tower under pale mist
foreground_slots:
  - position_x_ratio: 0.28
    subject_ref: courier-ren
    visibility: public
    height_meters: 1.74
    pose: standing three-quarters right with weight balanced forward
    left_hand:
      item: brass storm lantern with hexagonal glass panes
      action: holding aloft at shoulder height
    right_hand:
      item: null
      action: resting flat over leather satchel flap
    exclusive_surface_map:
      bindings:
        - slot: coat
          material: waxed canvas
          color_hex: "#2C3E50"
          finish: matte
        - slot: scarf
          material: coarse wool
          color_hex: "#B84A39"
style_ref: franco-belgian-line
atmosphere_ref: golden-dawn
action_summary: Courier raises a brass storm lantern while greeting an archivist on a timber pier
width: 1280
height: 720
```

## 3. Completion Criteria

Every authored or edited `SceneSpec` must pass deterministic compilation via:

- CLI: `imgforge validate <path-to-scene.yaml>`
- MCP: `validate` with `target: "scene_spec"` and `scene_path` or `scene_spec`.

When a `Blockout` `SpatialGuide` is paired, validate and render its `GuideSpec` (`schemaVersion: "1.0"`) with `guide` (`action: "render"`) and attach the rendered PNG via `--spatial-guide` / `spatial_guide_path`.
