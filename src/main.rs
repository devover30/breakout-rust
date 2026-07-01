// src/main.rs — rux-pivot strategy runner
//
// Strategy:
//   - Every 5-min boundary, read the latest CLOSED candle per symbol (from the lib).
//   - Read pivot levels from Redis key  pivot:latest:<symbol>  (JSON).
//   - close > R1  -> long  webhook  |  close < S1 -> short webhook.
//   - One order per symbol per day (tracked in memory).
//   - No signals before 09:20 IST, hard shutdown at 14:30 IST.

use chrono::{FixedOffset, NaiveTime, Utc};
use chrono_tz::Asia::Kolkata;
use rust_candle_fetcher::{Candle, PivotEngine};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;

// ---- IST logging --------------------------------------------------------

struct IstTimer;

impl FormatTime for IstTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap();
        let now = Utc::now().with_timezone(&ist);
        write!(w, "{}", now.format("%Y-%m-%d %H:%M:%S IST"))
    }
}

// ---- syms.json : { "<symbol>": { "short": "<url>", "long": "<url>" } } ----
#[derive(Deserialize)]
struct Urls {
    short: String,
    long: String,
}

// ---- Only the two fields we need out of the Redis pivot JSON ----
#[derive(Deserialize)]
struct Pivot {
    r1: f64,
    s1: f64,
}

// BigDecimal -> f64 (simple, dependency-free)
fn close_f64(c: &Candle) -> f64 {
    c.close.to_string().parse().unwrap_or(0.0)
}

// Sleep until just after the next 5-min wall-clock boundary.
// IST is UTC+5:30 (a multiple of 5 min), so UTC 5-min marks == IST 5-min marks.
async fn sleep_to_next_boundary() {
    let now = Utc::now().timestamp();
    let period = 300; // 5 minutes
    let next = ((now / period) + 1) * period;
    let secs = (next - now) as u64 + 3; // +3s so the closed candle is settled in the DB
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

// Read R1/S1 for one symbol from Redis. Returns None if missing/unparseable.
async fn get_pivot(con: &mut redis::aio::MultiplexedConnection, symbol: &str) -> Option<Pivot> {
    let key = format!("pivot:latest:{}", symbol);
    let raw: Option<String> = redis::cmd("GET").arg(&key).query_async(con).await.ok()?;
    serde_json::from_str::<Pivot>(&raw?).ok()
}

// Fire the Fyers webhook (plain POST, empty body). Returns true on success.
async fn fire(http: &reqwest::Client, url: &str) -> bool {
    match http.post(url).send().await {
        Ok(resp) => {
            tracing::info!("Order sent -> {} (status {})", url, resp.status());
        }
        Err(e) => {
            tracing::error!("Order failed -> {}: {}", url, e);
        }
    }
    true
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_timer(IstTimer).init(); // swap in your IstTimer subscriber if desired

    let db_url = std::env::var("DATABASE_URL")?;
    let redis_url = std::env::var("REDIS_URL")?;

    // syms.json lives in the project root
    let syms: HashMap<String, Urls> = serde_json::from_str(&std::fs::read_to_string("syms.json")?)?;
    let symbols: Vec<String> = syms.keys().cloned().collect();
    tracing::info!("Loaded {} symbols from syms.json", symbols.len());

    let engine = PivotEngine::new(&db_url, 5).await?;
    let redis_client = redis::Client::open(redis_url)?;
    let mut redis_con = redis_client.get_multiplexed_async_connection().await?;
    let http = reqwest::Client::new();

    let mut placed: HashSet<String> = HashSet::new();
    let start = NaiveTime::from_hms_opt(9, 30, 0).unwrap();
    let cutoff = NaiveTime::from_hms_opt(14, 30, 0).unwrap();

    loop {
        let now = Utc::now().with_timezone(&Kolkata).time();

        if now >= cutoff {
            tracing::info!("14:30 IST reached — hard shutdown.");
            std::process::exit(0);
        }
        if now < start {
            continue; // before the first tradeable candle
        }

        let candles = match engine.fetch_latest_candles(&symbols).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Candle fetch failed: {}", e);
                continue;
            }
        };

        for candle in candles {
            if placed.contains(&candle.symbol) {
                continue; // already traded this symbol today
            }

            let pivot = match get_pivot(&mut redis_con, &candle.symbol).await {
                Some(p) => p,
                None => {
                    tracing::warn!("No pivot in Redis for {}", candle.symbol);
                    continue;
                }
            };
            let urls = match syms.get(&candle.symbol) {
                Some(u) => u,
                None => continue,
            };

            let close = close_f64(&candle);

            if close > pivot.r1 {
                tracing::info!(
                    "{} LONG  (close {} > R1 {})",
                    candle.symbol,
                    close,
                    pivot.r1
                );
                if fire(&http, &urls.long).await {
                    placed.insert(candle.symbol.clone());
                }
            } else if close < pivot.s1 {
                tracing::info!(
                    "{} SHORT (close {} < S1 {})",
                    candle.symbol,
                    close,
                    pivot.s1
                );
                if fire(&http, &urls.short).await {
                    placed.insert(candle.symbol.clone());
                }
            }
        }
        sleep_to_next_boundary().await;
    }
}
