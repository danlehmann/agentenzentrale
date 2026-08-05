# Q — Agentenzentrale

A unified HTTPS **control plane + web UI** for managing coding-agent worker
machines. One web interface to reach all of them, built for being published
over the internet.

`Agentenzentrale` is German for "agent headquarters" — the single place every
field agent (worker) reports into. The tool is branded **Q**.

Currently supports [opencode](https://opencode.ai) workers; the agent layer is a
trait so other coding agents can be added later.

## What it does

- **One interface to rule them all.** From a single browser page you add
  workers, see their sessions, and chat with each agent.
- **Connects to opencode servers on your network.** Each worker just runs
  `opencode serve`; Q talks to it over HTTP (outbound). No SSH or config-file
  access on the workers is needed to add a new one — you do it from the UI.
- **HTTPS with login.** Q serves HTTPS (out of the box with a self-signed cert)
  and requires authentication. Sessions are cookie-based; invites let extra
  users join.
- **Extensible.** The [`AgentBackend`](src/agent/mod.rs) trait is the seam for
  plugging in future agent types, not just opencode.

## Architecture

```
                (browser)  ──HTTPS──►  Q (this program)
                                          │  outbound HTTP
              ┌───────────────────────────┼───────────────────────┐
              ▼                           ▼                       ▼
      worker A: opencode serve     worker A: opencode serve   worker B: opencode serve
            (agent 1)                 (agent 2, other project)    (agent 3)
```

- Each **worker** is a machine running `opencode serve --hostname 0.0.0.0`
  with a password set (`OPENCODE_SERVER_PASSWORD`).
- **Q** stores each worker's address + password (encrypted at rest) and dials
  them outbound. Add/remove workers from the web UI.
- Q does **not** run the LLMs. The heavy model hardware (your cluster) stays
  where it already is; Q is just the control and routing layer.

## Security model

Security is designed in from the start because the tool is meant to go public.

- **HTTPS everywhere.** TLS via rustls. On first run with no certificate, Q
  generates a self-signed cert — fine for testing, but for public exposure use
  a real certificate (e.g. put Caddy/Let's Encrypt in front, or supply
  `--cert`/`--key`).
- **Password hashing.** All account passwords stored with **Argon2id**.
- **Cookie sessions, hashed at rest.** Browser session tokens are random 32-byte
  values; only their SHA-256 hashes are stored. Cookies are `HttpOnly`,
  `SameSite=Lax`, and `Secure` when TLS is on.
- **CSRF protection.** Every mutating form carries a per-session CSRF token that
  is verified server-side.
- **Login rate limiting.** Per-IP throttling blocks brute force.
- **Worker secrets encrypted at rest.** Worker passwords are AES-256-GCM
  encrypted with a key in `data/.q-key` (mode 0600). Separate from the DB.
- **First-run admin bootstrap.** If no users exist, the first account created is
  an admin; everyone else joins via single-use invite links.
- **Agent output is sanitized.** Markdown rendered from agents (comrak + syntect
  for syntax highlighting) is run through **ammonia** to strip scripts, event
  handlers, and `javascript:` URLs before it reaches your browser. The only
  relaxation is the `style` attribute (needed for code highlighting), which
  cannot execute code.
- **Least-privilege workers.** Q authenticates to each worker with basic auth;
  workers need only enough permission to run `opencode serve` for their project.

## Getting started

```
cargo build --release
./target/release/q --addr 0.0.0.0:8443
```

Open `https://<host>:8443`, trust the self-signed cert (or configure a real
one), and it takes you to **setup** to create the first admin account.

Options (all also settable via `Q_*` env vars):

| Flag | Default | Purpose |
|------|---------|---------|
| `--addr` | `0.0.0.0:8443` | Listen address |
| `--data-dir` | `./data` | SQLite DB, secrets, certs |
| `--cert` / `--key` | auto | PEM TLS cert/key (omit to self-sign) |
| `--tls=false` | `true` | Disable TLS (e.g. behind a TLS reverse proxy) |
| `--public-url` | – | Base URL used when building invite links |

## Adding a worker (opencode)

1. On the worker machine, run opencode's server for a project:

   ```
   OPENCODE_SERVER_PASSWORD=<picker-a-strong-one> opencode serve --hostname 0.0.0.0 --port 4096
   ```

   Run one server per project/agent you want exposed (each on its own port, or
   in its own container).

2. In Q: **Workers → Add worker**, enter a name, the worker's URL
   (`http://<worker-ip>:4096`), and the same password.

That's it — no SSH, no config file. The worker is reachable from Q immediately.

## Sharing with more users

Admin → **Invites → Create invite link**, then send the link. The invitee
creates their own account (non-admin). Invites are single-use.

## Adding a new agent type (roadmap)

Implement the [`AgentBackend`](src/agent/mod.rs) trait
(`list_sessions`, `send_text`, `abort`, `events`, …) and register its `kind` in
[`AppState::backend_for`](src/web/mod.rs). The rest of Q (auth, UI, routing)
is agent-agnostic.

## Development

```
cargo test        # unit tests incl. HTML sanitization + crypto round-trip
cargo build --release
```

The UI is server-rendered (Askama + HTMX) for management pages; the chat/thread
page uses a small self-contained JS module (`static/chat.js`) for file
drag-and-drop (read client-side) and auto-refresh, with pretty server-side
markdown rendering. `static/htmx.min.js` is vendored (Zero-Clause BSD) so the
UI works offline.
