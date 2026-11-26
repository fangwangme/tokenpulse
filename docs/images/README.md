# Screenshots

The four images in this directory are generated from a deterministic synthetic
dataset. They do not contain a real account, real usage, or real quota values.

- `tui-usage.png` — TUI Usage, All range, dates list
- `tui-quota.png` — TUI Quota, all four providers
- `mobile-usage.png` — phone Usage, 7 days, By model
- `mobile-quota.png` — phone Quota, OpenCode Go card

## Regenerate

Requirements: Go 1.27+, Node 20+, tmux, Python 3, Playwright with Chromium, and
ImageMagick.

```bash
# 1. Seed a throwaway data directory with a year of synthetic usage and quota.
go run docs/images/generate/main.go -data-dir .local/data/tokenpulse

# 2. Build the phone app and the test binary.
cd web && npm run build && cd ..
go build -o .local/tokenpulse .

# 3. Capture the TUI in a 130x38 tmux pane.
tmux -L tokenpulse-final new-session -d -x 130 -y 38 -c "$PWD" bash
tmux -L tokenpulse-final send-keys -t 0 "env HOME=/tmp/tokenpulse-demo-home \
  XDG_DATA_HOME=/tmp/tokenpulse-demo-home/.local/share \
  TOKENPULSE_ENV=test TOKENPULSE_DATA_DIR=$PWD/.local/data/tokenpulse \
  TOKENPULSE_CONFIG_PATH=$PWD/.local/data/tokenpulse/config.toml \
  ./.local/tokenpulse --no-scan" Enter
tmux -L tokenpulse-final send-keys -t 0 '5'   # All range
tmux -L tokenpulse-final capture-pane -e -p -t 0 > .local/tui-usage.ansi
tmux -L tokenpulse-final send-keys -t 0 'Tab' # Quota tab
tmux -L tokenpulse-final capture-pane -e -p -t 0 > .local/tui-quota.ansi

# 4. Render the captures and take the phone screenshots through the test server.
python3 docs/images/generate/render_ansi.py .local/tui-usage.ansi .local/tui-usage.html
python3 docs/images/generate/render_ansi.py .local/tui-quota.ansi .local/tui-quota.html
# Open the two HTML files in a browser and screenshot .terminal, or use
# Playwright locator('.terminal').screenshot().
```

For the phone images, start `tokenpulse serve --no-tailscale` with the same
test environment, open the pairing URL from its banner in a phone viewport, and
enable subscription quota in Settings. Use 7 days and By model for the usage
image, and scroll to the OpenCode Go card for the quota image. Keep the
viewport at one phone screen; do not stitch a long page.
