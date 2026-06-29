use breakout_rust::{AppError, PivotEngine};
use chrono::{Local, NaiveTime, Timelike};
use chrono_tz::Asia::Kolkata;
use std::time::Duration;
use std::{env, process};
use tracing_subscriber::fmt::time::FormatTime;

// 1. Create a dedicated IST formatter struct
struct IstTimer;

impl FormatTime for IstTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        let now_ist = chrono::Local::now().with_timezone(&chrono_tz::Asia::Kolkata);
        write!(w, "{}", now_ist.format("%Y-%m-%d %H:%M:%S IST"))
    }
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    // Configure tracing-subscriber to format timestamps explicitly in Asia/Kolkata (IST)
    tracing_subscriber::fmt().with_timer(IstTimer).init();

    // 1. Read and validate environment variables natively
    let db_url = env::var("DATABASE_URL").map_err(|_| {
        AppError::Config("DATABASE_URL environment variable is missing".to_string())
    })?;

    let symbols_raw = env::var("FYERS_SYMBOLS").map_err(|_| {
        AppError::Config("FYERS_SYMBOLS environment variable is missing".to_string())
    })?;

    // Transform "SYMBOL1,SYMBOL2" string into Vec<String>
    let symbols: Vec<String> = symbols_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if symbols.is_empty() {
        return Err(AppError::Config("FYERS_SYMBOLS list is empty".to_string()));
    }

    tracing::info!("Symbols configured from Makefile: {:?}", symbols);

    // 1. Initialize the library engine object directly with configuration rules
    let engine = PivotEngine::new(&db_url, 5).await?;

    let market_start = NaiveTime::from_hms_opt(9, 20, 0).unwrap();
    let market_close = NaiveTime::from_hms_opt(15, 30, 0).unwrap();

    tracing::info!("Pivot engine initialized natively. Checking market window...");

    loop {
        let now_ist = Local::now().with_timezone(&Kolkata);
        let current_time = now_ist.time();

        if current_time < market_start || current_time > market_close {
            tracing::warn!(
                "Current time {} IST is outside active window. Exiting.",
                now_ist.format("%H:%M:%S")
            );
            break;
        }

        let current_minute = now_ist.minute();
        let current_second = now_ist.second();
        let next_minute = ((current_minute / 5) + 1) * 5;

        let minutes_to_wait = next_minute - current_minute;
        let total_seconds_to_wait = (minutes_to_wait * 60) - current_second;

        tracing::info!(
            "Next interval alignment loop in {} seconds...",
            total_seconds_to_wait
        );
        tokio::time::sleep(Duration::from_secs(total_seconds_to_wait as u64)).await;

        match engine.fetch_latest_candles(&symbols).await {
            Ok(candles) => {
                for candle in candles {
                    tracing::info!(
                        "[{}] Engine Metric -> O: {}, H: {}, L: {}, C: {}",
                        candle.symbol,
                        candle.open,
                        candle.high,
                        candle.low,
                        candle.close
                    );
                }
            }
            Err(e) => {
                tracing::error!("Error pulling engine logs: {}", e);
                process::exit(1);
            }
        }

        let post_fetch_time = Local::now().with_timezone(&Kolkata).time();
        if post_fetch_time >= market_close {
            tracing::info!("15:30 PM final interval met. Terminating app safely.");
            break;
        }
    }

    Ok(())
}
