# 0001. Rust Core Binary with Embedded Localhost Web Canvas UI

- **Status**: accepted

We will build `imgforge` as a single Rust binary (`axum` + `rmcp` + `image`/`palette`/`tiny-skia`/`resvg`) with an embedded TypeScript/HTML5 Canvas SPA (`rust-embed`) served on `127.0.0.1:<port>`, compiled inside a Docker `rust` + `node` builder container via `Makefile` and executed directly on the WSL2 host.

We rejected Tauri/egui desktop GUIs (fragile WebKitGTK/WSLg HiDPI and clipboard/drag-and-drop behavior on WSL2) and Python/OpenCV core libraries (slow MCP cold-start and per-repo virtualenv drift). Building in Docker keeps the WSL2 host free of language toolchains while running the compiled Linux binary natively on the host lets it spawn `codex exec` and bind `127.0.0.1` for the Windows browser without container-boundary friction.
