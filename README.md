# breakout-rust

A simple intraday R1/S1 breakout strategy runner built in Rust. It consumes
closed 5-minute candles from the [`rust-candle-fetcher`](../rust-candle-fetcher)
library, compares each candle's close against pivot levels stored in Redis, and
fires a per-symbol Fyers webhook when a breakout occurs.

Target timezone: **Asia/Kolkata (IST)**.

---

## How it works

1. **Candles** — `rust-candle-fetcher` exposes a `PivotEngine` that returns the
   latest 5-minute candle per symbol from TimescaleDB. The first tradeable
   candle of the day is `09:15–09:20`, available at `09:20 IST`.
2. **Pivots** — for each symbol the runner reads `pivot:latest:<symbol>` from
   Redis (a JSON string) and uses only the `r1` and `s1` fields.
3. **Signal** — evaluated on candle **close**:
   - `close > R1` → **long** webhook
   - `close < S1` → **short** webhook
   - touching a level (`==`) is **not** a breakout (strict `>` / `<`).
4. **One order per symbol per day** — once a symbol fires, it is recorded in
   an in-memory set and skipped for the rest of the run. The slot is claimed
   **before** the webhook is sent, so the order count never depends on the
   HTTP response.
5. **Time window** — no signals before `09:20 IST`; **hard shutdown at
   `14:30 IST`** (`std::process::exit(0)`).

The runner wakes a few seconds after each 5-minute wall-clock boundary,
fetches all symbols' latest candles in one pass, and evaluates them.

---

## Project layout

```
parent/
├── breakout-rust/          # this project (the strategy binary)
│   ├── src/main.rs
│   ├── syms.json
│   └── Cargo.toml
└── rust-candle-fetcher/    # the candle library (separate project)
```

`breakout-rust` depends on the sibling library by path:

```toml
[dependencies]
rust-candle-fetcher = { path = "../rust-candle-fetcher" }
```

> The binary uses only the library's public surface (`PivotEngine`, `Candle`).
> All database / `sqlx` mechanics stay hidden inside the library.

---

## Dependencies

```toml
[dependencies]
rust-candle-fetcher = { path = "../rust-candle-fetcher" }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
redis = { version = "0.27", features = ["tokio-comp"] }
reqwest = "0.12"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = "0.4"
chrono-tz = "0.10"
tracing = "0.1"
tracing-subscriber = "0.3"
```

---

## Configuration

### Environment variables

| Variable       | Purpose                                              |
| -------------- | ---------------------------------------------------- |
| `DATABASE_URL` | TimescaleDB / Postgres connection string (used by the library). |
| `REDIS_URL`    | Redis connection string, e.g. `redis://127.0.0.1:6379`. |

### `syms.json` (project root)

Maps each symbol to its two Fyers webhook URLs. The keys also define the
watchlist — these are the only symbols the runner trades.

```json
{
  "NSE:NIFTY50-INDEX": {
    "short": "https://fyers-webhook/.../nifty-short",
    "long":  "https://fyers-webhook/.../nifty-long"
  },
  "NSE:MIDCPNIFTY-INDEX": {
    "short": "https://fyers-webhook/.../midcp-short",
    "long":  "https://fyers-webhook/.../midcp-long"
  }
}
```

### Redis pivot format

Stored at key `pivot:latest:<symbol>` as a JSON string. Only `r1` and `s1`
are read; extra fields are ignored.

```json
{
  "symbol": "NSE:MIDCPNIFTY-INDEX",
  "date": "2026-06-30",
  "pivot": 14371.82,
  "r1": 14445.08,
  "r2": 14522.77,
  "s1": 14294.13,
  "s2": 14220.87,
  "prev_high": 14449.5,
  "prev_low": 14298.55,
  "prev_close": 14367.4,
  "timestamp": "2026-06-30T08:27:23.140467596+05:30"
}
```

---

## Running

```bash
export DATABASE_URL="postgres://user:pass@host:5432/db"
export REDIS_URL="redis://127.0.0.1:6379"

cargo run --release
```

Make sure `syms.json` is present in the project root and the pivot keys exist
in Redis before market open.

---

## Webhooks

Each breakout sends a **plain `POST` with an empty body** to the matching URL
from `syms.json`. The order is sent exactly once per symbol per run,
**regardless of the HTTP status returned** — the symbol is marked as traded
before the request is made.

Note: `reqwest` only surfaces transport failures (timeout, connection refused)
as errors. A `4xx`/`5xx` rejection from Fyers still arrives as a successful
response, so check the logged status if an order appears to have been ignored
downstream.

---

## Known limitations

- **In-memory dedup.** The "one order per symbol" set lives in memory only. A
  process restart during market hours re-arms every symbol, and running two
  instances at once will not share state. Acceptable for the short single-day
  window with a hard 14:30 stop.
- **Latest vs. closed candle.** If the library query returns the *current*
  in-progress 5-minute bucket, the close is not final. To always evaluate the
  just-closed candle, the library's query should exclude the live bucket:

  ```sql
  WHERE symbol = $1
    AND time_bucket('5 minutes', ltt) < time_bucket('5 minutes', now())
  ```

- **No persistence of fills.** The runner does not confirm or reconcile orders
  with Fyers; it only sends the webhook.
