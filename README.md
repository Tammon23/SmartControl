# SmartControl

A native Windows tray app and a Rust server for Linux for your existing Wi-Fi bulb. The Windows app is ready now; the server is packaged for later Oracle deployment. The optional Kindle plugin is maintained separately from this repository.

## Start with Windows

Open **windows/bulb-tray.exe**. The yellow bulb icon provides power, brightness presets, colour presets, and the Windows colour picker. Keep `libunwind.dll` and `client.json` beside it. See [Windows instructions](windows/START-HERE.md).

The application uses direct Rust HTTP requests and native sign-in renewal. It has no Node.js, Python, CLI or WebView dependency. Its existing authorization was migrated into a private local app-data file. The panel supports a bulb toggle, brightness slider, colour swatches and a saved local name.

## Build from source

Linux (OpenSSL development headers and pkg-config required):

```sh
cargo test --locked -p bulb-server
cargo build --locked --release -p bulb-server
```

Windows with a normal Rust MSVC toolchain and Visual Studio C++ Build Tools:

```powershell
cargo build --locked --release -p bulb-tray
```

The included Windows executable was built with Rust's GNU/LLVM toolchain; its included `libunwind.dll` is required. An MSVC build may have different runtime requirements. Release builds enable size optimization, LTO and symbol stripping. No exact size or latency guarantee is made.
