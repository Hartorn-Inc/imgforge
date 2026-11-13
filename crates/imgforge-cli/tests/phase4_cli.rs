//! === PHASE 4 CLI END-TO-END INTEGRATION TESTS ===
//!
//! PURPOSE:
//! Exercises unified `imgforge generate` (`--explore`, `--compose`, `--width`/`--height`,
//! `--spatial-guide`, multi-ref auto-`Atlas`), `imgforge validate` (`--run`, `--pack`),
//! `imgforge promote` (`--run`, `--pack`), `imgforge run`, and `imgforge library promote`
//! using isolated workspaces and a deterministic stub `codex` binary.

use std::fs;
use std::io::{BufRead, BufReader, Cursor, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba, RgbaImage};
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

fn write_studio_sheet_png(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(1536, 1024, Rgba([0x25, 0x28, 0x30, 255]));
    for (start_x, rgb) in [(200u32, [200u8, 120, 80]), (880u32, [90u8, 170, 210])] {
        for y in 140..760 {
            for x in start_x..(start_x + 220) {
                img.put_pixel(x, y, Rgba([rgb[0], rgb[1], rgb[2], 255]));
            }
        }
    }
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    fs::write(path, bytes).unwrap();
}

fn setup_workspace() -> (TempDir, PathBuf, PathBuf) {
    let root = TempDir::new().expect("tempdir");
    let ws = root.path().join("ws");
    fs::create_dir_all(ws.join("library/subjects/brass-lantern")).unwrap();
    fs::create_dir_all(ws.join("runs")).unwrap();
    fs::write(
        ws.join("imgforge.toml"),
        "schema_version = 1\nname = \"phase4-cli\"\n",
    )
    .unwrap();
    fs::write(
        ws.join("library/subjects/brass-lantern/card.yaml"),
        r##"schema_version: 1
kind: prop
slug: brass-lantern
display_name: Brass Lantern
exclusive_surface_map:
  bindings:
    - slot: body
      material: brushed brass
      color_hex: "#D4A843"
palette_lock:
  swatches:
    - name: brass
      color_hex: "#D4A843"
      surface_slot: body
visibility: public
"##,
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

fn write_smart_codex_stub(
    dir: &Path,
    scene_fixture: &Path,
    sheet_fixture: &Path,
    args_log: &Path,
) -> PathBuf {
    let stub = dir.join("codex-smart-stub");
    let script = format!(
        r##"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n---\n' "$*" >> "{args_log}"
SUBCMD="${{1:-}}"
if [ "$SUBCMD" = "exec" ] || true; then
  ALL_ARGS="$*"
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
    mkdir -p "$CODEX_HOME/generated_images/thr_p4"
    if printf '%s\n' "$ALL_ARGS" | grep -q "1536x1024"; then
      cp "{sheet_fixture}" "$CODEX_HOME/generated_images/thr_p4/out.png"
    else
      cp "{scene_fixture}" "$CODEX_HOME/generated_images/thr_p4/out.png"
    fi
    printf '{{"type":"thread.started","thread_id":"thr_p4"}}\n'
    exit 0
  fi

if [ -n "$OUT_PATH" ]; then
  if grep -q '"CriticReport"' "$(dirname "$OUT_PATH")/critic_report.schema.json" 2>/dev/null; then
    if printf '%s\n' "$ALL_ARGS" | grep -q "pose-color-identity"; then
      cat <<'JSON' > "$OUT_PATH"
{{"schema_version":1,"passed":true,"checks":[{{"trait_id":"pose-color-identity","category":"identity","passed":true,"observation":"poses match turnaround colors and identity"}},{{"trait_id":"zero-leaked-text","category":"zero_leaked_text","passed":true,"observation":"clean"}}],"summary":"all passed"}}
JSON
    elif printf '%s\n' "$ALL_ARGS" | grep -q "style-adherence"; then
      cat <<'JSON' > "$OUT_PATH"
{{"schema_version":1,"passed":true,"checks":[{{"trait_id":"zero-leaked-text","category":"zero_leaked_text","passed":true,"observation":"clean"}},{{"trait_id":"style-adherence","category":"style_adherence","passed":true,"observation":"matches style"}}],"summary":"all passed"}}
JSON
    else
      cat <<'JSON' > "$OUT_PATH"
{{"schema_version":1,"passed":true,"checks":[{{"trait_id":"zero-leaked-text","category":"zero_leaked_text","passed":true,"observation":"clean"}}],"summary":"all passed"}}
JSON
    fi
    exit 0
  fi
    cat <<'JSON' > "$OUT_PATH"
{{"schema_version":1,"id":"composed-harbor","title":"Composed Harbor","visibility":"public","camera":{{"shot":"medium 35mm","angle":null,"framing":null}},"depth_bands":[{{"plane":"foreground","near_meters":1.0,"far_meters":4.0,"description":"stone pier"}}],"foreground_slots":[],"style_ref":"franco-belgian-line","atmosphere_ref":"golden-dawn","action_summary":"Harbor pier bathed in morning sunlight","width":512,"height":512}}
JSON
    exit 0
  fi
fi
exit 2
"##,
        args_log = args_log.display(),
        scene_fixture = scene_fixture.display(),
        sheet_fixture = sheet_fixture.display(),
    );
    fs::write(&stub, script).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    stub
}

#[test]
fn cli_phase4_generate_explore_compose_validate_and_promote_e2e() {
    let (root, ws, data_home) = setup_workspace();
    let scene_fixture = root.path().join("scene_640.png");
    let sheet_fixture = root.path().join("sheet_1536x1024.png");
    write_png(&scene_fixture, 640, 640, [150, 155, 160, 255]);
    write_studio_sheet_png(&sheet_fixture);

    let args_log = root.path().join("codex_calls.log");
    let stub = write_smart_codex_stub(root.path(), &scene_fixture, &sheet_fixture, &args_log);

    let guide_path = ws.join("spatial_guide.png");
    write_png(&guide_path, 128, 128, [80, 100, 120, 255]);

    // 1. Compose + explore + width/height override before compile + spatial guide
    let gen_explore = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "--codex-bin",
            stub.to_str().unwrap(),
            "generate",
            "--compose",
            "Harbor pier bathed in morning sunlight",
            "--explore",
            "--width",
            "320",
            "--height",
            "320",
            "--spatial-guide",
            guide_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        gen_explore.status.success(),
        "generate --compose --explore failed: {}",
        String::from_utf8_lossy(&gen_explore.stderr)
    );
    let gen_json: serde_json::Value = serde_json::from_slice(&gen_explore.stdout).unwrap();
    assert_eq!(gen_json["run_id"], "composed-harbor/v001");
    assert_eq!(gen_json["lifecycle"], "candidate");
    assert!(
        gen_json["compiled_prompt"]
            .as_str()
            .unwrap()
            .contains("dimensions: 320x320")
    );
    assert!(
        gen_json["compiled_prompt"]
            .as_str()
            .unwrap()
            .contains("SpatialGuide layout reference")
    );

    // 2. Validate Run (candidate -> validated) with VisionCritic
    let val_run = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "--codex-bin",
            stub.to_str().unwrap(),
            "validate",
            "--run",
            "composed-harbor:1",
            "--atmosphere",
            "golden-dawn",
            "--critic",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        val_run.status.success(),
        "validate --run failed: {}",
        String::from_utf8_lossy(&val_run.stderr)
    );
    let val_json: serde_json::Value = serde_json::from_slice(&val_run.stdout).unwrap();
    assert_eq!(val_json["metadata"]["lifecycle"], "validated");

    // 3. Promote Run (validated -> canonical)
    let promo_run = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "promote",
            "--run",
            "composed-harbor:v001",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        promo_run.status.success(),
        "promote --run failed: {}",
        String::from_utf8_lossy(&promo_run.stderr)
    );
    let promo_json: serde_json::Value = serde_json::from_slice(&promo_run.stdout).unwrap();
    assert_eq!(promo_json["metadata"]["lifecycle"], "canonical");
    assert_eq!(promo_json["canonicalCount"], 1);

    // 4. Generate ReferencePack, validate it, and promote via `library promote`
    let pack_gen = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "--codex-bin",
            stub.to_str().unwrap(),
            "pack",
            "generate",
            "--subject",
            "brass-lantern",
            "--style",
            "franco-belgian-line",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        pack_gen.status.success(),
        "pack generate failed: {}",
        String::from_utf8_lossy(&pack_gen.stderr)
    );

    let val_pack = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "--codex-bin",
            stub.to_str().unwrap(),
            "validate",
            "--pack",
            "brass-lantern:franco-belgian-line",
            "--critic",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        val_pack.status.success(),
        "validate --pack failed: {}",
        String::from_utf8_lossy(&val_pack.stderr)
    );

    let promo_pack = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "library",
            "promote",
            "--subject",
            "brass-lantern",
            "--style",
            "franco-belgian-line",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        promo_pack.status.success(),
        "library promote failed: {}",
        String::from_utf8_lossy(&promo_pack.stderr)
    );

    // 5. Verify manifest.lock.json checksums
    let verify_lock = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "library",
            "verify-lock",
        ])
        .output()
        .unwrap();
    assert!(
        verify_lock.status.success(),
        "verify-lock failed: {}",
        String::from_utf8_lossy(&verify_lock.stderr)
    );
}

#[test]
fn cli_mcp_stdio_handshake_lists_8_tools_and_4_resources_and_rejects_non_loopback_http() {
    let (_root, ws, data_home) = setup_workspace();

    // 1. Non-loopback --http bind address must fail closed
    let bad_http = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--workspace",
            ws.to_str().unwrap(),
            "mcp",
            "--http",
            "0.0.0.0:3099",
        ])
        .output()
        .unwrap();
    assert!(
        !bad_http.status.success(),
        "0.0.0.0 bind address must fail closed"
    );

    // 2. Spawn `imgforge mcp` over stdio JSON-RPC
    let mut child = Command::new(imgforge_bin())
        .env("XDG_DATA_HOME", &data_home)
        .args(["--workspace", ws.to_str().unwrap(), "mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    fn send_and_read(
        stdin: &mut std::process::ChildStdin,
        stdout: &mut BufReader<std::process::ChildStdout>,
        msg: &str,
    ) -> serde_json::Value {
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("invalid JSON line '{line}': {e}"))
    }

    let init_resp = send_and_read(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"e2e","version":"1.0"}}}"#,
    );
    assert_eq!(init_resp["id"], 1);
    assert!(init_resp["result"]["capabilities"]["tools"].is_object());
    assert!(init_resp["result"]["capabilities"]["resources"].is_object());

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    stdin.flush().unwrap();

    let tools_resp = send_and_read(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    let tools = tools_resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 8);

    let res_list = send_and_read(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#,
    );
    let resources = res_list["result"]["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 4);

    let read_subj = send_and_read(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"imgforge://subjects"}}"#,
    );
    let subj_text = read_subj["result"]["contents"][0]["text"].as_str().unwrap();
    let subj_json: serde_json::Value = serde_json::from_str(subj_text).unwrap();
    assert_eq!(subj_json[0]["subject"]["slug"], "brass-lantern");

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "mcp stdio server should exit cleanly on EOF"
    );
}
