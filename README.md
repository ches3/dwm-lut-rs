# dwm-lut-rs

Applies 3D LUTs to the Windows desktop by hooking into DWM.

A Rust implementation based on [ledoge/dwm_lut](https://github.com/ledoge/dwm_lut), [lauralex/dwm_lut](https://github.com/lauralex/dwm_lut), and [ed1ii/dwm_lut_fixed](https://github.com/ed1ii/dwm_lut_fixed).

## Supported Versions

- Windows 11 24H2 (Build 26100)
- Windows 11 25H2 (Build 26200)

## Features

- Apply SDR and HDR LUTs per monitor
- Profiles for switching multi-monitor LUT assignments
- `.cube` and eeColor LUT formats
- CLI for LUT control

## Files

- `dwm-lut.exe`: GUI / tray (runs in the background)
- `dwm-lut-cli.exe`: CLI that interfaces with the background app
- `dwm_lut_hook.dll`: DLL to be injected into DWM

## Build

Requires the MSVC toolchain.

```text
cargo build --release
```

## License

[GPL-3.0-or-later](https://www.gnu.org/licenses/gpl-3.0.html)
