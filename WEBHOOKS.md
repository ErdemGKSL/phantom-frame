# Webhook Support in phantom-frame

Webhooks let each server block call an external HTTP endpoint on every incoming
request. Two modes are supported:

| Type | Behaviour |
|------|-----------|
| `blocking` | Request is held until the webhook responds. `2xx` → proceed. `3xx` → forward redirect to client (with `Location` header). Non-`2xx` → deny. Timeout/error → `503`. |
| `notify` | Fire-and-forget background POST. Request proceeds immediately regardless of outcome. |
| `cache_key` | Response body (plain text) is used as the cache key for this request. On failure or empty body, falls back to the default key. Works in Dynamic and PreGenerate mode. |

Webhooks fire **before cache reads**, so access control is enforced even when
the response would have been served from cache.

---

## TOML configuration

Webhooks are declared as a TOML array under the server block using
`[[server.NAME.webhooks]]`.

```toml
[server.default]
bind_to = "*"
proxy_url = "http://localhost:8080"

# Blocking webhook — gates access
[[server.default.webhooks]]
url = "http://auth-service.internal/check"
type = "blocking"
timeout_ms = 3000        # optional, default: 5000 ms

# Notify webhook — fire-and-forget audit log
[[server.default.webhooks]]
url = "http://logger.internal/access-log"
type = "notify"
```

### Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `url` | string | yes | — | The endpoint phantom-frame will POST to |
| `type` | `"blocking"` \| `"notify"` \| `"cache_key"` | no | `"notify"` | Controls webhook behaviour |
| `timeout_ms` | integer | no | `5000` | Max ms to wait for the webhook response |

---

## Request payload

phantom-frame sends a `POST` request with `Content-Type: application/json`.
The request body is **never consumed** by the webhook logic, so there is no
added latency from reading it.

```json
{
  "method": "GET",
  "path": "/dashboard",
  "query": "tab=overview",
  "headers": {
    "authorization": "Bearer eyJ...",
    "accept": "text/html",
    "host": "example.com",
    "user-agent": "Mozilla/5.0 ..."
  }
}
```

| Field | Description |
|-------|-------------|
| `method` | HTTP method in uppercase (`GET`, `POST`, …) |
| `path` | URL path, e.g. `/api/users/42` |
| `query` | Raw query string without the leading `?`, empty string if absent |
| `headers` | Flat `string → string` map of all request headers |

---

## Blocking webhook — detailed behaviour

```
Client → phantom-frame → [POST webhook] → 2xx?  → backend / cache
                                        ↓ 3xx
                                  return redirect to client (with Location)
                                        ↓ non-2xx
                                  return that status to client
                                        ↓ timeout / error
                                  return 503 to client
```

1. phantom-frame POSTs the payload to `url` and awaits the response.
2. **`2xx` (200–299)** — the request is allowed to continue normally.
3. **`3xx` (301, 302, 307, etc.)** — phantom-frame returns the redirect to the client with the `Location` header intact. Redirects are **not** followed internally; the browser/client receives the raw `3xx` and handles the navigation. This is the standard pattern for bouncing unauthenticated users to a login page.
4. **Non-`2xx`, non-`3xx`** — phantom-frame returns that exact status code to the client. The request never reaches the backend or the cache.
5. **Timeout or network error** — phantom-frame returns `503 Service Unavailable` to the client.

When multiple `blocking` webhooks are configured they run **sequentially**. The
first denial short-circuits the chain — subsequent webhooks are not called.

### Implementing a blocking webhook — redirect example (login bounce)

Return `302` with a `Location` header to redirect unauthenticated users:

```js
app.post('/check', (req, res) => {
  const token = (req.body.headers['authorization'] ?? '').replace('Bearer ', '');
  if (!isValidToken(token)) {
    return res.redirect(302, '/login');  // phantom-frame forwards this to the browser
  }
  res.status(200).end();
});
```

### Implementing a blocking webhook — deny example

Your endpoint must:

- Accept `POST` requests with a JSON body.
- Return any `2xx` status to allow the request.
- Return any non-`2xx` status to deny the request (that status code is
  forwarded to the original client).
- Respond within `timeout_ms` milliseconds (default 5000).

Minimal example in Node.js / Express:

```js
import express from 'express';
const app = express();
app.use(express.json());

app.post('/check', (req, res) => {
  const { method, path, headers } = req.body;
  const token = (headers['authorization'] ?? '').replace('Bearer ', '');

  if (!isValidToken(token)) {
    return res.status(403).json({ reason: 'invalid token' });
  }
  res.status(200).end();
});

app.listen(4000);
```

Minimal example in Rust / Axum:

```rust
use axum::{extract::Json, http::StatusCode, routing::post, Router};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
struct WebhookPayload {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
}

async fn check(Json(payload): Json<WebhookPayload>) -> StatusCode {
    let auth = payload.headers.get("authorization").map(|s| s.as_str()).unwrap_or("");
    if auth == "Bearer secret-token" {
        StatusCode::OK
    } else {
        StatusCode::FORBIDDEN
    }
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/check", post(check));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:4000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

---

## `cache_key` webhook — detailed behaviour

```
Client → phantom-frame → [POST webhook] → 2xx + body → use body as cache key
                                        ↓ non-2xx / empty body / error
                                  use default cache key (method:path?query)
```

1. phantom-frame POSTs the payload and awaits the response.
2. **`2xx` + non-empty body** — the trimmed response body is used as the cache
   key for this request (both cache read and cache write).
3. **`2xx` + empty body, non-`2xx`, timeout, or error** — the default cache key
   (`method:path?query`) is used. The request is **never denied** by a
   `cache_key` webhook.

### Use cases

- **Per-user caching** — return a key like `GET:/dashboard:user-42` so each
  user gets their own cached copy.
- **A/B buckets** — return a key that encodes the user's experiment group.
- **Subscription tier** — return different keys for free vs. paid users so they
  get different cached versions of the same page.

### Example server

```js
app.post('/cache-key', (req, res) => {
  const { method, path, headers } = req.body;
  const userId = getUserIdFromToken(headers['authorization']);
  // Return a plain-text cache key — phantom-frame uses this string verbatim
  res.type('text/plain').send(`${method}:${path}:user-${userId}`);
});
```

```rust
async fn cache_key(Json(payload): Json<WebhookPayload>) -> String {
    let user_id = extract_user_id(&payload.headers);
    format!("{}:{}:user-{}", payload.method, payload.path, user_id)
}
```

### PreGenerate mode caveat

In PreGenerate mode, snapshots are warmed up at startup using the **default**
cache key (`cache_key_fn`). If a `cache_key` webhook returns a different key at
request time, the lookup will miss (→ 404 or fallthrough). Only use `cache_key`
webhooks with PreGenerate mode if you also control how snapshots are stored
(e.g. by pre-generating with the same key via `add_snapshot`).

---

## Notify webhook — detailed behaviour

```
Client → phantom-frame ──────────────────────→ backend / cache
              │
              └─ tokio::spawn ──→ [POST webhook]  (background, not awaited)
```

1. phantom-frame spawns a background task and immediately continues serving
   the request.
2. The background task POSTs the payload to `url`.
3. Errors (timeout, connection refused, non-`2xx`) are logged as `WARN` but
   do **not** affect the client response.

Use-cases: audit logging, analytics, telemetry, cache warm-up triggers.

---

## Execution order

Webhooks execute in the order they are declared in the TOML file, **before**
any cache reads or backend proxying. The full pipeline for a request is:

```
1. WebSocket upgrade check (if Connection: Upgrade)
2. forward_get_only check
3. Webhooks (in declaration order)
   3a. notify     → spawned in background, continue immediately
   3b. cache_key  → await; override cache key (or fall back to default on failure)
   3c. blocking   → await; 2xx = proceed, 3xx = forward redirect, non-2xx = deny, timeout = 503
4. Cache read (if applicable, using resolved cache key)
5. Backend proxy (on cache miss)
6. Cache write (if applicable, using resolved cache key)
7. Response to client
```

---

## Library API (programmatic usage)

When using phantom-frame as a library rather than the binary, pass webhooks
via `CreateProxyConfig::with_webhooks()`:

```rust
use phantom_frame::{CreateProxyConfig, WebhookConfig, WebhookType};

let config = CreateProxyConfig::new("http://localhost:8080".into())
    .with_webhooks(vec![
        // Gate access; redirect unauthenticated users to /login
        WebhookConfig {
            url: "http://auth.internal/check".into(),
            webhook_type: WebhookType::Blocking,
            timeout_ms: Some(3000),
        },
        // Audit log (fire-and-forget)
        WebhookConfig {
            url: "http://logger.internal/log".into(),
            webhook_type: WebhookType::Notify,
            timeout_ms: None,
        },
        // Per-user cache key
        WebhookConfig {
            url: "http://key-service.internal/cache-key".into(),
            webhook_type: WebhookType::CacheKey,
            timeout_ms: Some(500),
        },
    ]);

let (router, _handle) = phantom_frame::create_proxy(config);
```

---

## Tips & gotchas

- **Security before performance**: webhooks fire before cache reads. Even a
  cached `200` response will be denied if the blocking webhook says no.
- **Keep blocking webhooks fast.** Since they block the request pipeline,
  aim for sub-100 ms response times. Set `timeout_ms` aggressively (e.g. `500`)
  if you can guarantee your auth service is co-located.
- **Do not rely on notify for auth enforcement.** Notify webhooks are
  fire-and-forget; a slow or crashed webhook server goes unnoticed by the client.
- **Secrets in headers**: The full request header map is included in the payload
  (including `Authorization`, `Cookie`, etc.). Make sure your webhook URL is an
  internal / private endpoint — never expose it to the public internet.
- **`cache_key` webhooks are non-fatal**: a slow or unavailable `cache_key`
  webhook falls back silently to the default key — it never blocks or denies
  the request.
- **`cache_key` + PreGenerate**: snapshot warm-up uses the default `cache_key_fn`,
  not the webhook. Override keys only work at request time; see the PreGenerate
  caveat section above.
- **`timeout_ms` on notify webhooks**: accepted but only controls how long the
  background task waits before logging a warning — no impact on client response time.
- **Multiple blocking webhooks**: all must return `2xx` for the request to be
  allowed. The chain short-circuits on the first non-`2xx`/`3xx` response.
