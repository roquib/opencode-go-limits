# opencode-go-limits

Scrapes [OpenCode Go](https://opencode.ai/docs/go/) model usage limits
(requests per 5 hours / week / month) into a local TOML snapshot.

The snapshot lives at `~/.config/usages_limit/models.toml`, keyed by date
(keeps the last 3 days). Each model entry carries the display `name` plus the
authoritative `slug` (model ID) for use with `opencode -m opencode-go/<slug>`.

## Features

- **Hourly snapshot** — scrapes `https://opencode.ai/docs/go/` and writes
  `~/.config/usages_limit/models.toml` (keeps the last 3 days, keyed by date).
- **Go-plan scoping** — reads the `go` requests panel (`#go-requests-panel-go`,
  ~3x below `go-plus`), with fallback to any
  `Model | Requests per 5 hours | ...` table on older page layouts.
- **Authoritative slugs** — resolves each display name through the Endpoints
  table (`Model | Model ID`) to the exact `opencode-go/<slug>` ID, with
  normalization (promo suffixes, `MiMo V2.5` vs `MiMo-V2.5`) and prefix matching.
- **Promo & Unlimited aware** — promo cells with two figures (base + 4x boost)
  record the max; `Unlimited`/`∞` windows serialize as `"Unlimited"`.
- **Sorted output** — models sorted by 5h limit descending, `Unlimited` first.
- **Two modes** — `--once` for a single scrape; no flag loops hourly.
  `install.sh` wires the hourly cron job (logs to
  `~/.local/share/opencode-go-limits.log`).

Pairs with
[`opencode-model-picker`](https://github.com/roquib/opencode-model-picker):
an `Alt+M` fzf picker that reads this TOML. See that repo for the picker
screenshot and zsh setup.

## Install

```bash
./install.sh
# verbose output:
./install.sh --verbose
```

This builds the release binary, installs it to `~/.local/bin/opencode-go-limits`,
and adds an hourly cron job (logs to `~/.local/share/opencode-go-limits.log`).

> Requires `~/.local/bin` on your `PATH`. Check with
> `echo $PATH | grep -q "$HOME/.local/bin" && echo OK || echo MISSING`.
> If missing, add it once:
>
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc && source ~/.bashrc
> # zsh: echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc
> ```

## Usage

```bash
opencode-go-limits --once   # single scrape, then exit
opencode-go-limits          # scrape every hour (loop)
```

Example output file (`~/.config/usages_limit/models.toml`):

```toml
["2026-10-25"]
[[models."2026-10-25"]]
name = "Muse Spark 1.3 Contributor"
slug = "muse-spark-1.3-contributor"
requests_per_5h = "Unlimited"
requests_per_week = 100
requests_per_month = "Unlimited"
```

`Unlimited` windows are stored as the string `"Unlimited"`; limited windows
are integers. Promo cells with two figures (base + boost) record the max.

## Development

```bash
cargo test
cargo clippy -- -D warnings
cargo fmt --check
cargo build --release
```

## How it works

1. Fetches `https://opencode.ai/docs/go/`.
2. Reads the Endpoints table (`Model | Model ID | ...`) for authoritative slugs.
3. Reads the `go`-plan requests table under `#go-requests-panel-go`
   (falls back to any `Model | Requests per 5 hours | ...` table).
4. Sorts models by 5h limit descending (`Unlimited` first) and writes TOML.

## License

MIT — see [LICENSE](LICENSE).
