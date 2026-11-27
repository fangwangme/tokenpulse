# Remote Mode

`tokenpulse serve` runs on the computer, keeps usage and quota fresh whether or
not anyone looks, and serves a mobile web app so a phone on the same tailnet
(or, on explicit opt-in, the same LAN) shows usage, quota, settings and the
Keeper. The TUI stays a full client of the same data.

## Serve

`tokenpulse serve [--port <n>] [--https-port <n>] [--no-tailscale] [--lan]
[--web-dir <dir>] [--quiet]`

- **Single runtime lock.** Every runtime host (`serve` or an in-process TUI)
  holds an OS-level `flock` on `<data dir>/runtime.lock` for its lifetime. A
  second `serve` exits with status 1; a second TUI attaches over the local
  Unix socket.
- **Refresh ownership.** The runtime host is the sole writer that scans usage,
  fetches quota, schedules Keeper pings, and fires quota-recovery alerts.
  `serve` refreshes immediately on start, then every `display.auto_refresh_secs`
  (falling back to 900 seconds when set to 0). On start it warms its usage and
  quota caches from disk so clients see cached data immediately. Scans and
  fetches never run on HTTP request paths; `/api/usage*` and `/api/quota` read
  only the cache.
- **Requested refresh** (`POST /api/refresh` or TUI `r`) runs the requested
  work and waits for it so the next read sees fresh data without resetting the
  periodic timer. Each target (`usage`, `quota`) has a 60-second cooldown: a
  second request inside that window returns `200` with `accepted:false`,
  `throttled:true`, and the cached timestamps.
- **Config.** Settings and Keeper writes from the phone and TUI are serialized
  through the runtime and saved atomically to `config.toml`. The runtime also
  re-reads `config.toml` before each scheduled refresh, so manual or CLI edits
  take effect on the next cycle.
- **Updates.** After every refresh, settings write, Keeper run, or reprice,
  the runtime publishes an event to connected WebSocket and local clients and
  clears the report cache (`usage-report.md` "Caching"). Quota recovery emits
  its own event so the TUI can display a toast.
- Logging writes to the daily rotating log file, never stdout after the startup
  banner (`--quiet` suppresses the banner for background launches).

## Local control

Both `serve` and an in-process TUI runtime listen on `<data dir>/serve.sock`
(mode 0600). The socket speaks the same API without token auth — OS file
permissions are the trust boundary — plus local-only `GET /api/config`,
`POST /api/reprice`, and `POST /api/shutdown`. A second TUI and CLI commands
(`reprice`, `remote status`) attach through it; a stale socket that refuses
connections is replaced on start.

A Unix socket path is limited by the kernel (107 bytes on Linux, 103 on macOS).
When `<data dir>/serve.sock` does not fit, `serve` creates a private `0700`
directory `tokenpulse-<hash>` (hash of the absolute data directory) inside
`$XDG_RUNTIME_DIR`, or the system temporary directory when that is unset or the
candidate still does not fit, and puts `serve.sock` inside it with mode 0600.
The TUI and CLI resolve the path with the same rule, so a long
`TOKENPULSE_DATA_DIR` changes neither how `serve` starts nor how they find it.

## Desktop TUI

- The TUI is a thin frontend over `runtime.Client`. On start, if `runtime.lock`
  is already held, it attaches over `serve.sock`; otherwise it starts an
  in-process runtime (also listening on `serve.sock` without opening a TCP
  port).
- All seven views (`tui-usage.md`) read from the runtime and subscribe to
  live events; `r`, settings edits, and Keeper pings send commands
  to the runtime.
- Settings includes a **Web remote** toggle that starts or stops a detached
  background `tokenpulse serve --quiet` (or reports when `systemd`/`launchd`
  manages it). If a detached `serve` stops while the TUI is open, the TUI
  automatically falls back to an in-process runtime once `runtime.lock` frees.

## HTTP API

JSON in and out; errors are `{ "error": "<message>" }` with 400 for an invalid
parameter, 401 unauthenticated, 403 a rejected origin, 404 unknown, 409 a
conflicting in-flight action, 429 rate limited.

| Method | Path | Answer |
| :--- | :--- | :--- |
| POST | `/api/pair` | body `{ token }`: exchanges the remote token for a session cookie (Authentication); the only unauthenticated API call |
| GET | `/api/state` | `version`, `scannedAt`, quota `fetchedAt`, `pricesAvailable`, enabled `agents`, `settings` (the phone-editable values), `keeperEngine` (boolean: the Keeper master switch) |
| GET | `/api/status` | `running`, `scannedAt`, `fetchedAt`, `managedBy`, `webUrl`, `webError`; answers immediately without waiting for a scan |
| GET | `/api/usage?preset=&from=&to=&agents=` | the report of `usage-report.md` |
| GET | `/api/usage/day?date=&agents=` | the day detail |
| GET | `/api/usage/model?model=&from=&to=&agents=` | the model detail |
| GET | `/api/quota` | `{ providers: [{ provider, enabled, snapshot, error: { message, at } \| null }], fetchedAt }`, ordered and shaped per `quota.md`; always the cache, never a provider call |
| POST | `/api/refresh` | body `{ usage?: bool, quota?: bool }` (both default true); waits for the work and answers `{ accepted, throttled, retryAfter, scannedAt, fetchedAt }`; inside the 60-second cooldown it answers `200` with `accepted:false` and leaves the cache untouched |
| GET | `/api/settings` | the phone-editable settings with their allowed values |
| PATCH | `/api/settings` | partial update of phone-editable settings; answers the new settings |
| GET | `/api/keeper` | per agent: engine state, daily wakeup and weekly toggles and time, weekly targets with last-fired time, and the newest executions (time, agent, trigger, status, model, reply excerpt) |
| POST | `/api/keeper/ping` | body `{ agent, group? }`: the same manual ping as `p` on the Keeper tab; 400 for an agent the Keeper does not drive, a `group` on an agent other than Antigravity, or a group that is not one of its current quota groups; 409 while that key's ping is in flight |
| GET | `/api/push/key` | the VAPID public key |
| POST / DELETE | `/api/push/subscription` | store or remove this browser's push subscription |
| GET | `/ws` | server events: `usage_updated { scannedAt }`, `quota_updated { fetchedAt }`, `quota_recovered { provider, window, remaining }`, `keeper_updated`, `settings_updated`; session cookie only |

- `agents` is comma-separated; omitted means every enabled source. Dates are
  `YYYY-MM-DD`; anything else is 400.
- The API names Antigravity `agy`, on input and output; every other agent keeps
  its TokenPulse name. Inside the program the name stays `antigravity`.
- Every other path serves the web app; unknown app routes answer its
  `index.html`.

### What the phone may change

The phone may change exactly these keys, each checked against its own rule:

| Key | Accepted values |
| :--- | :--- |
| `display.quota_display_mode` | `used`, `remaining` |
| `display.auto_refresh_secs` | 60, 120, 300, 600, 900 |
| `display.account_display` | the values `config set account_display` accepts |
| `display.refresh_quota`, `display.scan_antigravity`, `display.show_empty_providers` | boolean |
| `display.notification_level` | the values `config set notification_level` accepts |
| `display.notification_sound` | the TUI's fixed sound list |
| `display.quota_provider_order` | non-empty list of unique quota provider IDs (missing IDs append in default order) |
| `providers.<id>.enabled` | boolean, for `claude`, `codex`, `opencode`, `agy` |
| `keeper.enabled` (the Keeper master switch) | boolean |
| `keeper.agents.<agent>.session_keeper_enabled`, `weekly_keeper_enabled` | boolean, for agents the Keeper drives |
| `keeper.agents.<agent>.daily_wakeup_time` | `HH:MM`, 00:00–23:59 |
| `keeper.agents.agy.weekly_all_groups` | boolean |

Wakeup time and Antigravity all-groups are new on the phone (the TUI edits
them only through the config file and a global setting); the rules above are
their validation.

It may never change a value that names a program, a file or a prompt:
`keeper.agents.*.command`, `keeper.agents.*.prompt` and `providers.*.path` are
refused with 400, as is any key or value not in the table. A request is
validated whole before anything is written: one refused entry refuses the
request and leaves `config.toml` byte-identical. A phone that could set the
Keeper's command could run any program on the computer.

Writes go to `config.toml` as it is on disk, the way the Settings tab writes,
and an invalid file on disk refuses the write instead of overwriting it.

### Never returned

No response carries a credential, an access or refresh token, the remote token,
an absolute file path, a Keeper command or prompt, or raw log text. A Keeper
execution's reply is returned only as an excerpt with absolute paths under the
home directory replaced by `~`. The remote token and session tokens never
appear in the log.

## Network access

Default ports: local HTTP `4120`, Tailscale HTTPS `8444`. They differ from the
earlier companion app's `4100`/`8443` so both can run.

Startup resolves one access mode, in this order:

1. **Tailscale Serve** (default). `tailscale status --json` gives the machine's
   DNS name and Tailscale IPv4. `tailscale serve status --json` is checked for
   the HTTPS port: when `/` already proxies to `127.0.0.1:<port>` it is used;
   when the port is unused, `serve` runs `tailscale serve --bg
   --https=<https-port> <port>`. `serve` binds `127.0.0.1` only, and the URL is
   `https://<dns-name>:<https-port>/` (no port when 443).
2. **Tailscale IP.** When Serve is unavailable, or the HTTPS port proxies to
   something else (never overwritten), `serve` binds the Tailscale IPv4 and
   prints a plain-HTTP tailnet URL, the reason, and the command that would
   enable Serve.
3. **Localhost.** Without Tailscale, `serve` binds `127.0.0.1` and says the
   phone cannot reach it.

`--no-tailscale` skips 1 and 2. `--lan` skips 1 and 2 as well (it never
configures Tailscale Serve), binds all interfaces, prints the LAN URL, and
warns that LAN traffic is unencrypted. TokenPulse never terminates TLS itself.
A test run (`go-rewrite.md`) uses its own ports and never configures or
changes a Tailscale Serve mapping on a production port.

Requests are accepted only with a `Host` naming localhost, `127.0.0.1`, the
Tailscale DNS name or IP, or, with `--lan`, the LAN addresses; anything else is
403.

Startup prints, on the terminal only, the URL with the token in its fragment
and a QR code of it, the access mode, and any recovery command.

## Authentication

- The first `serve` creates `remote.json` in the data directory (mode 0600)
  holding a random 128-bit remote token.
- The pairing URL carries the token in the fragment (`/#token=<token>`), so it
  never reaches a request line, a log or a `Referer`. The app reads it, removes
  it from the address bar (`history.replaceState`), and sends it once to
  `POST /api/pair`.
- `/api/pair` is POST-only, compares in constant time and is limited to 10
  attempts per minute per remote IP (never globally). A match creates a random
  session token, stores only its SHA-256 hash, and answers with a cookie holding
  the session token (`HttpOnly`, `SameSite=Strict`, `Path=/`, 400-day
  lifetime, `Secure` whenever the access mode is HTTPS — Tailscale Serve —
  whatever the proxy headers say). The remote token itself never appears in a
  response; a mismatch is 401. At most 20 sessions are kept and older ones are
  evicted.
- `/api/*` and `/ws` need a valid session cookie, or `Authorization: Bearer
  <remote token>` for non-browser clients; otherwise 401. A token in a query
  string is never accepted. The web app shell loads without auth and shows a
  "scan the QR code" page when its first API call is 401.
- Every state-changing request (POST, PATCH, DELETE) and every WebSocket
  upgrade authenticated by cookie must carry an `Origin` whose scheme, host and
  effective port match the request and an accepted host; a missing or foreign
  `Origin` is 403. Only Bearer-authenticated requests may omit `Origin`. The
  local Unix socket is trusted and skips this check.
- `remote.json` is written with a temp file, fsync and rename. A corrupt file is
  reported and left untouched instead of silently generating a new token.
- `tokenpulse remote reset-token` writes a new remote token and deletes every
  session; existing cookies are 401 at once and live phone sockets close on
  their next session check. `tokenpulse remote url` prints the pairing URL and
  QR code again.

## Web Push

- `serve` creates VAPID keys on first start, stored in the data directory with
  mode 0600. Subscriptions are stored next to them; a subscription the push
  service answers with 404 or 410 is deleted.
- A push is sent for exactly the quota-recovery events that fire the desktop
  notification, with the same text, and only while `display.notification_level`
  is not `off`. No other event pushes.
- iOS delivers web push only to an app added to the home screen; the Settings
  page says so when push is unavailable.

## Web app

A mobile-first single-page app under `web/` (Solid + TypeScript + Vite),
built into the binary at build time; `serve --web-dir <dir>` serves a directory from disk for
development. The embedded directory always contains a committed placeholder
page saying the frontend was not built, which the web build replaces, so a
plain `go build` without Node succeeds and the API works unchanged.

### Mobile PWA shell

The document does not declare `apple-mobile-web-app-status-bar-style`: an
installed iOS Home Screen app starts below an opaque status bar in the page's
color, so the system does not blur content scrolling under it.
`viewport-fit=cover` stays in the viewport meta, safe areas use
`env(safe-area-inset-*)`, and the initial `theme-color` follows the light or
dark palette background.

The manifest requests `portrait`, and an installed Android app also calls
`screen.orientation.lock("portrait")` on start where the browser allows it.
iOS supports neither, so an iPhone keeps rotating and its landscape layout
stays usable; TokenPulse does not block it.

The window never scrolls. `html`, `body` and `#root` are fixed to the layout
viewport with `overflow: hidden`; a page fills that viewport with a fixed
frame, and only its inner scroll area scrolls. The page's bottom safe-area
padding lives inside that scroll area. In standalone mode the bottom tab bar's
background fills the bottom safe area while its controls stay above the home
indicator. The refresh action is a floating accent button above the tab bar;
it hides and returns with the bar while the page scrolls. The layout uses no
`100dvh`/`100lvh`: on iOS 26 those units can
include the status-bar height and leave a fixed empty band under the bottom
controls.

While a text field has focus, the page is sized and placed to the visual
viewport, so the header stays visible and the page's content stays above the
keyboard; with no text field focused the visual-viewport readings are ignored
and the window stays at scroll position 0. iOS reads the status-bar style when
the app is added to the Home Screen, so an already installed app picks up a
change only after it is removed and added again.

The Usage and Quota pages are a component-by-component port of the reference
mobile design, not a reinterpretation: layout, spacing, type scale, color
tokens, heatmap shapes, bottom sheets, skeleton and error states, and visible
wording follow that design. The Usage page opens on `today`. In a period the
list card has one `By date | By model` segmented switch; date and model are not
two separate entries. The heatmap keeps its three shapes (`strip`,
`week-rows`, `week-columns`), the 24 px `week-columns` cells, Monday-only date
labels, opening at the newest week, drag scrolling and no zoom.

The app shell is the bottom tab bar Usage, Quota, Keeper and Settings, with
safe-area padding at the bottom and centered at 520 px on wide screens. The bar
owns page switching, so the pages carry no top `Quota | Usage` switch, no
back button and no large repeated title; Usage and Quota open with a compact
scanned/refreshed line and the refresh button inside the scroll area.

The app knows all seven agents (`claude`, `codex`, `copilot`, `gemini`,
`opencode`, `pi`, `agy`) with a name and color each, and shows those the
server lists as enabled.

- **Pages**: Usage (`usage-report.md` presentation: range presets with custom
  dates, KPI cards with comparison in periods, heatmap with active/current/
  longest streaks and a five-step legend, date/model list of 10 rows expanding
  by 20, day and model sheets, agent filter, Token/Cost switch), Quota
  (`quota.md`: per provider card with plan and account per `account_display`,
  ordered windows with bar, pace, reset time, credits, extra usage, error next
  to the last snapshot), Keeper (status, toggles, manual ping, recent
  executions), Settings (the editable settings, theme, language, push).
  Keeper and pairing have no counterpart in the reference design and use the
  same components and tokens. Settings presents the quota providers as one
  ordered list: each row has a drag handle and an enable switch, dragging from
  the handle saves the order, and the switch saves independently.
- The page header is the compact status line (last scan/refresh time) and the
  refresh button, not a second title.
- A phone that has paired before renders the app shell without waiting for the
  API: a stored pairing marker decides this, the bottom tab bar and the selected
  page's frame appear before `/api/state` resolves, and the pairing page appears
  only after a 401 or when no marker exists. The first screen starts
  `/api/state`, the `today` usage report, and `/api/quota` together instead of
  waiting for one another. The last successful state and `today` usage report
  are kept in versioned local storage; a later launch renders them first and
  replaces them when the fresh responses arrive. A stored usage report is used
  only when its preset and agent set match the requested view, and a 401 clears
  the marker and every stored first-screen value. A transient failure keeps the
  shell and the stored values visible instead of returning to pairing. Only the
  opening `today` view is stored; other ranges and filtered views are not. The
  service worker remains push-only and caches no API data, and the document
  carries no pre-rendered shell.
- Pages update silently on `usage_updated` / `quota_updated`; the socket
  reconnects with backoff, and failed or unloaded data is re-requested once it
  is back. A failed load shows the error with a retry button, never an endless
  spinner.
- Light, dark and system themes, chosen on the phone and stored in the
  browser. Chinese and English, following the browser language.
- A web app manifest and icons make it installable; the service worker handles
  push only and caches no API data.

## Service

`tokenpulse service install | uninstall | status`

- Linux: a systemd user unit `tokenpulse.service` running the absolute path of
  the current executable with `serve`, `Restart=on-failure`; `install` enables
  and starts it, `uninstall` stops, disables and removes it.
- macOS: a launchd agent in `~/Library/LaunchAgents/` with `RunAtLoad` and
  `KeepAlive`, loaded with `launchctl bootstrap`, removed with `bootout`.
- `status` prints whether the unit is installed and whether `serve` answers on
  the local socket.
