# minacalc-overlay

Tosu overlay + MinaCalc difficulty sidecar for **osu!mania**.

[MSD](https://community.etternaonline.com/t/what-is-msd/2265)(Mina Standardized Difficulty) is a comprehensive 4-key difficulty standard. Use this overlay to get a better picture of the mania-beatmap difficulty.
- Rust binary polls tosu’s HTTP endpoints and writes `msd.json`.
- Overlay (`overlay/*`) renders those values.

## Quick start

```bash
# Build
cargo build --release

# (Optional) point to tosu.env explicitly
#   --tosu-env D:\tosu\tosu.env      # Windows
#   --tosu-env ~/tosu/tosu.env         # macOS/Linux

# Run
./target/release/minacalc-overlay --tosu-env ./tosu/tosu.env
```


This tool reads **`tosu.env`** and uses the value of **`STATIC_FOLDER_PATH`** to determine where the overlay should live. If `STATIC_FOLDER_PATH` is relative, it's resolved relative to the folder containing `tosu.env`.

- If `tosu.env` can't be found, we fall back to using the local `./overlay` directory (development mode).
- You can override the path to `tosu.env` via `--tosu-env` or environment variable `TOSU_ENV_PATH` (for this binary only).

On first run, if the overlay isn't already installed,  copy the contents of `./overlay` into:
```
<STATIC_FOLDER_PATH>/MinaCalcOnOsu/
```

After launching **tosu**, open `http://127.0.0.1:24050/` and select `MinaCalcOnOsu` in the dashboard.

## License

MIT (see `LICENSE`).

## Credits

- [`minacalc-rs`](https://crates.io/crates/minacalc-rs) — MIT
- [tosu](https://github.com/tosuapp/tosu) — LGPL-3.0 (consumed via HTTP)
