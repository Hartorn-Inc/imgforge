//! === CLI END-TO-END INTEGRATION TESTS ===
//!
//! PURPOSE:
//! Exercises the compiled `imgforge` CLI binary across `auth`, `status`,
//! `compose`, `validate`, `generate`, and `critic` using an isolated temp
//! workspace (`imgforge.toml`), temp `$XDG_DATA_HOME`, and a deterministic
//! stub `codex` executable.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
use rstest::rstest;
use tempfile::TempDir;

fn bin_path() -> &'static str {
    env!("CARGO_BIN_EXE_imgforge")
}

fn write_png_file(path: &std::path::Path, width: u32, height: u32) {
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(width, height, Rgba([80, 140, 200, 255]));
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("encode PNG");
    fs::write(path, bytes).expect("write PNG file");
}

fn create_stub_codex(dir: &std::path::Path, fixture_png: &std::path::Path) -> std::path::PathBuf {
    let stub = dir.join("codex-e2e-stub");
    let script = format!(
        r##"#!/usr/bin/env bash
set -euo pipefail
SUBCMD="${{1:-}}"
if [ "$SUBCMD" = "login" ]; then
  if [ "${{2:-}}" = "status" ]; then
    test -f "$CODEX_HOME/auth.json"
    echo "Authenticated via OAuth stub"
    exit 0
  fi
  printf '{{"token":"e2e-stub-token"}}\n' > "$CODEX_HOME/auth.json"
  exit 0
fi

if [ "$SUBCMD" = "exec" ]; then
  OUT_PATH=""
  IS_JSONL=0
  for arg in "$@"; do
    if [ "$arg" = "--json" ]; then
      IS_JSONL=1
    fi
  done
  while [ "$#" -gt 0 ]; do
    case "$1" in
      -o) OUT_PATH="$2"; shift 2 ;;
      *) shift ;;
    esac
  done

  if [ "$IS_JSONL" -eq 1 ]; then
    mkdir -p "$CODEX_HOME/generated_images/thr_e2e"
    cp "{fixture}" "$CODEX_HOME/generated_images/thr_e2e/out.png"
    printf '{{"type":"thread.started","thread_id":"thr_e2e"}}\n'
    exit 0
  fi

  if [ -n "$OUT_PATH" ]; then
    if grep -q '"CriticReport"' "$(dirname "$OUT_PATH")/critic_report.schema.json" 2>/dev/null; then
      cat <<'JSON' > "$OUT_PATH"
{{"schema_version":1,"passed":true,"checks":[{{"trait_id":"hand-lantern","category":"hand_props","passed":true,"observation":"brass lantern visible in left hand"}}],"summary":"passed"}}
JSON
      exit 0
    fi
    cat <<'JSON' > "$OUT_PATH"
{{"schema_version":1,"id":"e2e-dock","title":"E2E Harbor Pier","visibility":"public","camera":{{"shot":"medium 35mm","angle":"eye-level","framing":null}},"depth_bands":[{{"plane":"foreground","near_meters":1.0,"far_meters":4.0,"description":"stone pier"}}],"foreground_slots":[{{"position_x_ratio":0.5,"subject_ref":"courier-ren","visibility":"public","height_meters":1.75,"pose":"standing steady","left_hand":{{"item":"brass lantern","action":"holding"}},"right_hand":{{"item":null,"action":"at side"}},"exclusive_surface_map":{{"bindings":[{{"slot":"coat","material":"wool","color_hex":"#2C3E50","finish":"matte"}}]}}}}],"style_ref":"ink-wash","atmosphere_ref":"dawn-mist","action_summary":"Courier stands on a stone pier holding a brass lantern","width":512,"height":512}}
JSON
    exit 0
  fi
fi

echo "unexpected stub invocation: $*" >&2
exit 3
"##,
        fixture = fixture_png.display()
    );
    fs::write(&stub, script).expect("write stub codex");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod stub");
    stub
}

#[test]
fn cli_full_phase1_pipeline_auth_compose_validate_generate_critic_and_status() {
    let root = TempDir::new().expect("tempdir");
    let ws_dir = root.path().join("workspace");
    let xdg_dir = root.path().join("xdg");
    fs::create_dir_all(&ws_dir).unwrap();
    fs::create_dir_all(&xdg_dir).unwrap();

    let fixture_png = root.path().join("gen_600.png");
    let ref_png = root.path().join("ref.png");
    write_png_file(&fixture_png, 600, 600);
    write_png_file(&ref_png, 128, 128);

    let stub = create_stub_codex(root.path(), &fixture_png);
    fs::write(
        ws_dir.join("imgforge.toml"),
        format!(
            "schema_version = 1\nname = \"e2e-ws\"\n[backend]\ncodex_bin = \"{}\"\n",
            stub.display()
        ),
    )
    .unwrap();

    // 1. Missing auth fails closed on auth status
    let unauth_status = Command::new(bin_path())
        .args(["--workspace", ws_dir.to_str().unwrap(), "auth", "status"])
        .env("XDG_DATA_HOME", &xdg_dir)
        .output()
        .expect("run auth status");
    assert!(!unauth_status.status.success());

    // 2. Login provisions tool-local auth.json
    let login = Command::new(bin_path())
        .args([
            "--workspace",
            ws_dir.to_str().unwrap(),
            "auth",
            "login",
            "--device-auth",
        ])
        .env("XDG_DATA_HOME", &xdg_dir)
        .output()
        .expect("run auth login");
    assert!(
        login.status.success(),
        "login stderr: {}",
        String::from_utf8_lossy(&login.stderr)
    );

    // 3. Status succeeds via IMGFORGE_WORKSPACE env precedence
    let ws_status = Command::new(bin_path())
        .args(["status", "--json"])
        .env("IMGFORGE_WORKSPACE", &ws_dir)
        .env("XDG_DATA_HOME", &xdg_dir)
        .output()
        .expect("run status --json");
    assert!(ws_status.status.success());
    let status_json: serde_json::Value =
        serde_json::from_slice(&ws_status.stdout).expect("parse status JSON");
    assert_eq!(status_json["auth_provisioned"], serde_json::json!(true));
    assert_eq!(
        status_json["resolution_source"],
        serde_json::json!("env_var")
    );

    // 4. Compose prose -> validated YAML SceneSpec
    let scene_yaml = root.path().join("scene.yaml");
    let compose = Command::new(bin_path())
        .args([
            "--workspace",
            ws_dir.to_str().unwrap(),
            "compose",
            "--prose",
            "Courier on stone pier with brass lantern",
            "--out",
            scene_yaml.to_str().unwrap(),
        ])
        .env("XDG_DATA_HOME", &xdg_dir)
        .output()
        .expect("run compose");
    assert!(
        compose.status.success(),
        "compose stderr: {}",
        String::from_utf8_lossy(&compose.stderr)
    );
    assert!(scene_yaml.is_file());

    // 5. Validate SceneSpec deterministically
    let validate = Command::new(bin_path())
        .args(["validate", scene_yaml.to_str().unwrap()])
        .env("XDG_DATA_HOME", &xdg_dir)
        .output()
        .expect("run validate");
    assert!(validate.status.success());

    // 6. Generate resampled 512x512 PNG from raw 600x600 stub output
    let out_png = root.path().join("generated.png");
    let generate = Command::new(bin_path())
        .args([
            "--workspace",
            ws_dir.to_str().unwrap(),
            "generate",
            "--scene",
            scene_yaml.to_str().unwrap(),
            "--out",
            out_png.to_str().unwrap(),
            "-i",
            ref_png.to_str().unwrap(),
        ])
        .env("XDG_DATA_HOME", &xdg_dir)
        .output()
        .expect("run generate");
    assert!(
        generate.status.success(),
        "generate stderr: {}",
        String::from_utf8_lossy(&generate.stderr)
    );
    let generated_img = image::open(&out_png).expect("open generated PNG");
    assert_eq!((generated_img.width(), generated_img.height()), (512, 512));

    // 7. VisionCritic evaluates generated image against reference
    let critic = Command::new(bin_path())
        .args([
            "--workspace",
            ws_dir.to_str().unwrap(),
            "critic",
            "--image",
            out_png.to_str().unwrap(),
            "-i",
            ref_png.to_str().unwrap(),
            "--trait",
            "hand-lantern:hand_props:Left hand holds brass lantern",
        ])
        .env("XDG_DATA_HOME", &xdg_dir)
        .output()
        .expect("run critic");
    assert!(
        critic.status.success(),
        "critic stderr: {}",
        String::from_utf8_lossy(&critic.stderr)
    );

    // 8. Logout removes auth.json
    let logout = Command::new(bin_path())
        .args(["auth", "logout"])
        .env("XDG_DATA_HOME", &xdg_dir)
        .output()
        .expect("run auth logout");
    assert!(logout.status.success());
    assert!(!xdg_dir.join("imgforge/codex/auth.json").exists());
}

#[rstest]
#[case::unknown_subcommand(&["does-not-exist"])]
#[case::invalid_explicit_workspace(&["--workspace", "/nonexistent/imgforge/ws", "status"])]
fn cli_invalid_invocations_fail_closed(#[case] args: &[&str]) {
    let output = Command::new(bin_path())
        .args(args)
        .output()
        .expect("spawn imgforge");
    assert!(!output.status.success());
}
