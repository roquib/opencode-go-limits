# opencode-go-limits

Scrapes the [OpenCode Go](https://opencode.ai/console/go#limits) console page
and records each model's estimated requests per 5 hours plus its monthly
allowance into a local TOML snapshot.

The snapshot lives at `~/.config/usages_limit/models.toml`, keyed by date
(keeps the last 3 days). Each model entry carries the display `name`, the
authoritative `slug` (model ID) for `opencode -m opencode-go/<slug>`, and the
5-hour request budget.

## Features

- **Hourly snapshot** — writes `~/.config/usages_limit/models.toml`
  (keeps the last 3 days, keyed by date).
- **Reads the console, not the docs** — the docs page no longer publishes the
  request table; this uses `https://opencode.ai/console/go`, scraping the
  server-rendered limits table (`#limits` is only a client-side anchor).
- **Authoritative slugs** — taken straight from each row's `data-model`
  attribute, so IDs always match `opencode -m opencode-go/<slug>`
  (no name-to-slug guessing).
- **Plan selection** — the console renders Go and Go Plus in one table;
  `--plan go` (default) or `--plan go-plus` picks which column set to record.
- **Unlimited aware** — `∞` / `Unlimited` budgets serialize as `"Unlimited"`.
- **Sorted output** — models sorted by 5h budget descending, `Unlimited` first.
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
opencode-go-limits --once              # single scrape, then exit
opencode-go-limits                     # scrape every hour (loop)
opencode-go-limits --once --plan go-plus   # record Go Plus limits instead
opencode-go-limits --help
```

Example output file (`~/.config/usages_limit/models.toml`):

```toml
[[models.2026-11-26]]
name = "DeepSeek V4.1 Flash 1017"
slug = "deepseek-v4.1-flash"
requests_per_5h = 43300
monthly_limit = "$100"

[[models.2026-11-26]]
name = "Beam Free"
slug = "beam-free"
requests_per_5h = "Unlimited"
```

`Unlimited` budgets are stored as the string `"Unlimited"`; finite ones are
integers. `monthly_limit` is the plan's spend allowance as published by the
console (`"$15"`, `"$100"`, …) and is omitted when the plan is unlimited.

## Development

```bash
cargo test
cargo clippy -- -D warnings
cargo fmt --check
cargo build --release
```

## How it works

1. Fetches `https://opencode.ai/console/go` (no login required today; a
   non-200 response is reported as a hard error).
2. Selects `tr[data-model]` rows — `data-model` is the authoritative slug.
3. Reads the display name from `[data-layer="model-name"]` (`title` attr).
4. Reads the requested plan's values from
   `td[data-allowance-type="requests"]` and `td[data-allowance-type="usage"]`,
   matching `[data-plan="go"|"go-plus"] [data-layer="current-value"]`.
5. Parses `∞`/`Unlimited`/comma-grouped numbers, sorts by 5h budget
   descending, and writes the TOML snapshot.

If the console layout changes and no rows are found, the run fails loudly
instead of writing an empty snapshot.

## License

MIT — see [LICENSE](LICENSE).
