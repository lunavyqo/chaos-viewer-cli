# Claims coordination API (generic)

`chaos` does **not** hardcode a claims host. Any project may point at **any**
coordinator by publishing:

```json
{
  "project": {
    "claimsApi": "https://your-coordinator.example/api/claims",
    "claimsAuthUrl": "https://your-coordinator.example/auth/github/start"
  }
}
```

- Omit both → no live API (CLI still supports `CLAIMS.md` in the decomp repo).
- `claimsApi` is the **list** endpoint base used for reads and write suffixes.
- `claimsAuthUrl` is optional browser OAuth; the CLI prefers API keys / env tokens.

This contract matches [chaos-viewer](https://github.com/tangosdev/chaos-viewer)
`ADAPTING.md` and the agent instructions served by coordinators such as
`belongto.us` (one popular implementation for sm64ds — not required).

## Read path (no key)

```
GET {claimsApi}
→ { "ok": true, "claims": [ { "module", "start", "end", "handle", "id"? } ] }
```

`start` / `end` may be numbers or `"0x…"` strings. Ranges are typically half-open
`[start, end)`.

Also supported: merge active rows from the decomp repo’s `CLAIMS.md` on
`main`/`master` (markdown table). That path needs no service at all.

## Write path (key required)

Send `X-Api-Key: <token>` on every write.

| Method | Path | Body |
|--------|------|------|
| POST | `{claimsApi}/try-lock` | `{ "module", "start": "0x…", "end": "0x…", "handle", "note"? }` |
| POST | `{claimsApi}/{id}/renew` | `{ "handle" }` |
| POST | `{claimsApi}/{id}/release` | `{ "handle" }` |

Success responses should be JSON with `"ok": true` and often a `claim` object
including `id`. Conflicts may return non-2xx with an `error` / `conflicts` field.

Optional docs URL: `GET {claimsApi}/instructions` (markdown or plain text).

## Getting a key (coordinator-defined)

Implementations differ. Common patterns (all supported by this CLI):

1. **Long-lived API key** (Discord bot, admin dashboard, …) → set env var.
2. **Browser GitHub OAuth** via `claimsAuthUrl?redirect=…` → short-lived session
   (web dashboards; CLI can use the resulting token if you paste it).
3. **GitHub token exchange** (if implemented):  
   `POST {origin}/auth/github/token` with `{ "accessToken": "<gh token>" }`  
   → `{ "ok": true, "session", "handle" }`.

## CLI environment

```bash
export CHAOS_CLAIMS_API_KEY='…'      # or CHAOS_CLAIMS_SESSION / CHAOS_CLAIMS_KEY
export CHAOS_CLAIMS_HANDLE='your-name'
```

The TUI also persists a session under `~/.config/chaos/claims-session.toml`
(created by sign-in / paste). Env vars override that file when set.

## TUI (same write path as the web viewer)

With a loaded atlas that publishes `project.claimsApi` (sm64ds →
`https://tangos.dev/api/claims`):

| Key | Action |
|-----|--------|
| **`i`** (Claims page) | Sign in: try `gh auth token` → `POST /auth/github/token`, else paste Discord key / session |
| **`o`** | Sign out (clears saved session) |
| **`L`** | Claim selected function (`POST …/try-lock`) |
| **`A`** | Claim every function in the active mass-batcher slot |
| **`y`** | Renew my claims |
| **`x`** | Release my claims |
| **`r`** | Refresh lock list |

Notes use `via chaos-viewer-cli: <name>`. Claim ids are stored for renew/release.

## CLI commands

```bash
# List locks (API + CLAIMS.md), using claimsApi from the atlas project block
chaos claims list --repo https://github.com/you/your-decomp

# Agent write flow (requires key env vars)
chaos claims try-lock --module arm9 --start 0x2000000 --end 0x2000100 --note 'matching'
chaos claims renew --id clm_…
chaos claims release --id clm_…

# Coordinator docs
chaos claims instructions --repo …

# Optional: exchange a GitHub token if the coordinator supports it
chaos claims github-exchange --github-token gho_… --api https://host/api/claims
```

Override the API base without loading an atlas:

```bash
chaos claims list --api https://your-coordinator.example/api/claims
```

## Building your own coordinator

You can run any service that implements the table above. Point
`project.claimsApi` at it in the published `chaos-db.json`. The CLI and
chaos-viewer will talk to it the same way — no special casing for any vendor.
