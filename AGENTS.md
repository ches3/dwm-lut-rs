# dwm-lut-rs

Applies 3D LUTs to the Windows desktop by hooking into DWM.

## Upstream

Based on these projects:

https://github.com/ledoge/dwm_lut
https://github.com/lauralex/dwm_lut
https://github.com/ed1ii/dwm_lut_fixed

## Verification Commands

- Run these commands after making changes and before committing.
- Do not ignore any errors or warnings.

```
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo check --all-targets --all-features
cargo test --all-features
```

## Log

`C:\Windows\Temp\dwm-lut-rs\hook-debug.log`
