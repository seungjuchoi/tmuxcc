#!/usr/bin/env bash
# Build + install tmuxcc from this checkout, then render it once in a throwaway
# tmux session and print the frame so the UI can be verified without attaching.
#
#   ./scripts/install-and-verify.sh          # install and show a frame
#   ./scripts/install-and-verify.sh --no-ui  # install only
set -euo pipefail

cd "$(dirname "$0")/.."

SESSION="tmuxcc-verify"
WIDTH="${TMUXCC_VERIFY_WIDTH:-200}"
HEIGHT="${TMUXCC_VERIFY_HEIGHT:-50}"

cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --quiet
cargo install --path . --force

echo
echo "installed: $(command -v tmuxcc) -> $(tmuxcc --version)"

if [ "${1:-}" = "--no-ui" ]; then
    exit 0
fi

if ! tmux ls >/dev/null 2>&1; then
    echo "tmux is not running; skipping the UI frame."
    exit 0
fi

tmux kill-session -t "$SESSION" 2>/dev/null || true
tmux new-session -d -s "$SESSION" -x "$WIDTH" -y "$HEIGHT" tmuxcc
sleep 3
echo
tmux capture-pane -p -t "$SESSION"
tmux kill-session -t "$SESSION"
