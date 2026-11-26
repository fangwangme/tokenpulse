# Screenshots

The two images in this directory show the mobile web dashboard. They are
generated from a deterministic synthetic dataset and do not contain a real
account, real usage, or real quota values.

- `mobile-usage.png` — phone Usage, first screen, 7 days, By model
- `mobile-quota.png` — phone Quota, first screen

## Regenerate

Requirements: Go 1.27+, Node 20+, Playwright with Chromium, and a phone-sized
browser viewport.

```bash
# 1. Seed a throwaway data directory with a year of synthetic usage and quota.
go run docs/images/generate/main.go -data-dir .local/data/tokenpulse

# 2. Build the web app and the test binary.
cd web && npm run build && cd ..
go build -o .local/tokenpulse .

# 3. Start the test server with an isolated home directory.
HOME=/tmp/tokenpulse-demo-home \
XDG_DATA_HOME=/tmp/tokenpulse-demo-home/.local/share \
TOKENPULSE_ENV=test \
TOKENPULSE_DATA_DIR=$PWD/.local/data/tokenpulse \
TOKENPULSE_CONFIG_PATH=$PWD/.local/data/tokenpulse/config.toml \
./.local/tokenpulse serve --no-tailscale
```

Open the pairing URL from the banner in a 393×852 phone viewport and enable
subscription quota in Settings. Use 7 days and By model for the usage image.
Both images are the first screen with the bottom tab bar visible; do not scroll
and do not stitch a long page.

The demo config starts with `display.refresh_quota=false` and
`display.scan_antigravity=false`, so the test server does not contact a real
provider or Antigravity Language Server. Enabling them in the phone Settings
UI only changes the config for the synthetic data; the next scheduled scan is
15 minutes away.
