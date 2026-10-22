# SmartControl

A native Windows tray app and a Rust server for Linux for your existing Wi-Fi bulb. The Windows app is ready now; the server is packaged for later Oracle deployment. The optional Kindle plugin is maintained separately from this repository.

## Start with Windows

Download the Windows ZIP from [Releases](https://github.com/Tammon23/SmartControl/releases), extract it, and run **SmartControl.exe**. Connect your account and select your bulb when setup opens. See [Windows instructions](windows/START-HERE.md).

The application uses direct Rust HTTP requests and native sign-in renewal. It has no Node.js, Python, CLI or WebView dependency. On first launch, connect your account in the browser and select a bulb. Credentials are encrypted for your Windows user account. The panel supports a bulb toggle, brightness slider, colour swatches and a saved local name.

Local automations support daily times and sunset offsets, with optional brightness and custom colours. Windows supplies the timezone; sunset coordinates are entered manually. Right-click the tray bulb for the Start at sign-in setting.

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

The release workflow builds a portable Windows x64 executable with MSVC and a static C runtime. Release builds enable size optimization, LTO and symbol stripping. See [release instructions](RELEASING.md) to publish a new compiled download.
