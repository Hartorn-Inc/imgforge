//! === PHASE 3 CLI END-TO-END INTEGRATION TESTS ===
//!
//! PURPOSE:
//! Exercises `imgforge edit`, `imgforge guide`, and `imgforge export` via the
//! compiled `imgforge` binary using isolated temporary workspaces and a
//! deterministic stub `codex` binary (never calling live `codex exec`).

use std::fs;
use std::io::Cursor;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba, RgbaImage};
use rstest::rstest;
use tempfile::TempDir;

fn imgforge_bin() -> &'static str {
    env!("CARGO_BIN_EXE_imgforge")
}

fn write_png(path: &Path, w: u32, h: u32, fill: [u8; 4]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let img: RgbaImage = ImageBuffer::from_pixel(w, h, Rgba(fill));
    let mut buf = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .unwrap();
    fs::write(path, buf).unwrap();
}

fn setup_workspace() -> (TempDir, PathBuf, PathBuf) {
    let root = TempDir::new().expect("tempdir");
    let ws = root.path().join("ws");
    fs::create_dir_all(ws.join("library/subjects/courier-ren")).unwrap();
    fs::create_dir_all(ws.join("library/atmospheres/dawn")).unwrap();
    fs::create_dir_all(ws.join("runs")).unwrap();
    fs::write(
        ws.join("imgforge.toml"),
        "schema_version = 1\nname = \"cli-e2e\"\n",
    )
    .unwrap();
    fs::write(
        ws.join("library/atmospheres/dawn/card.yaml"),
        r#"schema_version: 1
slug: dawn
display_name: Dawn
key_light_direction: warm low east light
secondary_light_direction: cool zenith fill
color_temperature_k: 3600
max_ambient_tint_blend: 0.25
exposure_floor:
  min_mean_l_star: 35.0
  min_p10_l_star: 12.0
weather_and_particles: clear air
"#,
    )
    .unwrap();

    let data_home = root.path().join("xdg_data");
    let auth_dir = data_home.join("imgforge/codex");
    fs::create_dir_all(&auth_dir).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&auth_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let auth_file = auth_dir.join("auth.json");
    fs::write(&auth_file, r#"{"token":"oauth-stub"}"#).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&auth_file, fs::Permissions::from_mode(0o600)).unwrap();

    (root, ws, data_home)
}

fn write_stub_codex(dir: &Path, name: &str, fixture_png: &Path) -> PathBuf {
    let script_path = dir.join(name);
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
mkdir -p "$CODEX_HOME/generated_images/thr_cli"
cp "{}" "$CODEX_HOME/generated_images/thr_cli/out.png"
printf '{{"type":"thread.started","thread_id":"thr_cli"}}\n'
"#,
        fixture_png.display()
    );
    fs::write(&script_path, script).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[test]
fn cli_guide_validate_and_render_blockout_and_overlay_e2e() {
    let (_root, ws, data_home) = setup_workspace();
    write_png(&ws.join("runs/base.png"), 128, 128, [80, 90, 110, 255]);

    let blockout_path = ws.join("blockout.json");
    fs::write(
        &blockout_path,
        r##"{
  "schemaVersion": "1.0",
  "id": "guide-cli-01",
  "title": "CLI Blockout",
  "variant": "Blockout",
  "targetImagePath": null,
  "canvasWidth": 256,
  "canvasHeight": 256,
  "depthBands": [
    { "id": "bg", "label": "Horizon", "plane": "background", "yStartRatio": 0.0, "yEndRatio": 0.5, "fillHex": "#2C3344" },
    { "id": "fg", "label": "Stage", "plane": "foreground", "yStartRatio": 0.5, "yEndRatio": 1.0, "fillHex": "#4A5164" }
  ],
  "mannequins": [
    { "id": "m1", "subjectSlug": "courier-ren", "label": "Ren", "xRatio": 0.5, "yRatio": 0.85, "heightMeters": 1.75, "scaleFactor": 1.0, "facing": "front", "poseHint": "standing", "paletteHex": "#63B3ED" }
  ],
  "actionVectors": [
    { "id": "v1", "label": "Move", "fromXRatio": 0.2, "fromYRatio": 0.7, "toXRatio": 0.8, "toYRatio": 0.3, "strokeHex": "#F6AD55", "haloHex": "#1A1D24", "strokeWidthPx": 4, "style": "solid" }
  ],
  "overlayRegions": []
}"##,
    )
    .unwrap();

    let val_out = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args(["--workspace", ws.to_str().unwrap(), "guide", "validate"])
        .arg(&blockout_path)
        .output()
        .unwrap();
    assert!(
        val_out.status.success(),
        "guide validate failed: {}",
        String::from_utf8_lossy(&val_out.stderr)
    );

    let out_png = ws.join("exports/blockout.png");
    let render_out = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "guide",
            "render",
            "--spec",
        ])
        .arg(&blockout_path)
        .arg("--out")
        .arg(&out_png)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        render_out.status.success(),
        "guide render failed: {}",
        String::from_utf8_lossy(&render_out.stderr)
    );
    let rendered = image::open(&out_png).unwrap().to_rgba8();
    assert_eq!((rendered.width(), rendered.height()), (256, 256));
}

#[test]
fn cli_edit_patch_and_export_with_visibility_gate_e2e() {
    let (root, ws, data_home) = setup_workspace();
    let target_path = ws.join("target.png");
    write_png(&target_path, 128, 128, [70, 95, 120, 255]);

    let fixture_patch = root.path().join("patch_fixture.png");
    write_png(&fixture_patch, 80, 80, [180, 130, 85, 255]);
    let stub_codex = write_stub_codex(root.path(), "codex-patch-stub", &fixture_patch);

    let patch_out = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "--codex-bin",
            stub_codex.to_str().unwrap(),
            "edit",
            "patch",
            "--target",
            target_path.to_str().unwrap(),
            "--slug",
            "harbor-patch",
            "--prompt",
            "warm lantern glow",
            "--visibility",
            "internal",
            "--roi",
            "48,48,32,32",
            "--prep",
            "cue-outline",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        patch_out.status.success(),
        "edit patch failed: {}",
        String::from_utf8_lossy(&patch_out.stderr)
    );

    let parsed_patch: serde_json::Value = serde_json::from_slice(&patch_out.stdout).unwrap();
    assert_eq!(parsed_patch["runId"], "harbor-patch/v001");
    assert_eq!(parsed_patch["outOfRoiMaxDeltaE2000"], 0.0);

    // Exporting internal Run to public profile MUST fail closed.
    let blocked_export = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "export",
            "--run",
            "harbor-patch:1",
            "--profile",
            "vtt-token",
            "--visibility",
            "public",
            "--out-dir",
            ws.join("exports/pub").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !blocked_export.status.success(),
        "internal Run exported to public must fail closed"
    );

    // Exporting internal Run to internal vtt-token + vtt-map succeeds.
    let ok_export = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "export",
            "--run",
            "harbor-patch:v001",
            "--profile",
            "vtt-token",
            "--visibility",
            "internal",
            "--token-size",
            "256",
            "--out-dir",
            ws.join("exports/int").to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        ok_export.status.success(),
        "internal export failed: {}",
        String::from_utf8_lossy(&ok_export.stderr)
    );
    assert!(ws.join("exports/int/harbor-patch_token.png").is_file());
}

#[rstest]
#[case::good_exposure_passes([160, 165, 170, 255], true)]
#[case::crushed_black_fails_closed([3, 3, 4, 255], false)]
fn cli_edit_whole_enforces_atmosphere_exposure_floor_fail_closed(
    #[case] backend_fill: [u8; 4],
    #[case] expect_ok: bool,
) {
    let (root, ws, data_home) = setup_workspace();
    let target_path = ws.join("scene.png");
    write_png(&target_path, 64, 64, [150, 155, 160, 255]);

    let fixture = root.path().join("whole_fixture.png");
    write_png(&fixture, 64, 64, backend_fill);
    let stub_codex = write_stub_codex(root.path(), "codex-whole-stub", &fixture);

    let out = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "--codex-bin",
            stub_codex.to_str().unwrap(),
            "edit",
            "whole",
            "--target",
            target_path.to_str().unwrap(),
            "--slug",
            "dawn-scene",
            "--prompt",
            "maintain golden dawn light",
            "--atmosphere",
            "dawn",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.success(), expect_ok);
}
