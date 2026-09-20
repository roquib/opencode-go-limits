#!/usr/bin/env bash
# Build a release binary and install it to ~/.local/bin/opencode-scraper.
# Also set up a cron job to run every hour.
set -euo pipefail

VERBOSE=false
for arg in "$@"; do
    case "$arg" in
        -v|--verbose) VERBOSE=true ;;
    esac
done

log() {
    if [[ "$VERBOSE" == true ]]; then
        echo "[verbose] $*"
    fi
}

cd "$(dirname "$0")"
log "Building release binary..."
cargo build --release

log "Installing binary to $HOME/.local/bin/opencode-scraper"
install -Dm755 target/release/opencode-scraper "$HOME/.local/bin/opencode-scraper"
echo "installed: $HOME/.local/bin/opencode-scraper"

log "Setting up cron job to run every hour..."
if crontab -l 2>/dev/null | grep -q "opencode-scraper"; then
    echo "Cron job already exists, skipping..."
else
    (crontab -l 2>/dev/null; echo "0 * * * * $HOME/.local/bin/opencode-scraper --once >> $HOME/.local/share/opencode-scraper.log 2>&1") | crontab -
    echo "Cron job added: runs every hour at minute 0"
fi

echo ""
echo "Installation complete!"
echo "  Binary: ~/.local/bin/opencode-scraper"
echo "  Cron job: Every hour (logs to ~/.local/share/opencode-scraper.log)"
echo "  Run manually: opencode-scraper --once"