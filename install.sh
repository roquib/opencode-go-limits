#!/usr/bin/env bash
# Build a release binary and install it to ~/.local/bin/opencode-go-limits.
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

log "Installing binary to $HOME/.local/bin/opencode-go-limits"
install -Dm755 target/release/opencode-go-limits "$HOME/.local/bin/opencode-go-limits"
echo "installed: $HOME/.local/bin/opencode-go-limits"

log "Setting up cron job to run every hour..."
# Migration from pre-rename installs: drop the old opencode-scraper cron entry.
if crontab -l 2>/dev/null | grep -q "opencode-scraper"; then
    log "Removing legacy opencode-scraper cron entry..."
    crontab -l 2>/dev/null | grep -v "opencode-scraper" | crontab -
    echo "Removed legacy opencode-scraper cron entry"
fi
if crontab -l 2>/dev/null | grep -q "opencode-go-limits"; then
    echo "Cron job already exists, skipping..."
else
    (crontab -l 2>/dev/null; echo "0 * * * * $HOME/.local/bin/opencode-go-limits --once >> $HOME/.local/share/opencode-go-limits.log 2>&1") | crontab -
    echo "Cron job added: runs every hour at minute 0"
fi

echo ""
echo "Installation complete!"
echo "  Binary: ~/.local/bin/opencode-go-limits"
echo "  Cron job: Every hour (logs to ~/.local/share/opencode-go-limits.log)"
echo "  Run manually: opencode-go-limits --once"