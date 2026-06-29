use chrono::{Local, NaiveTime, Timelike};
use chrono_tz::Asia::Kolkata;
use sqlx::postgres::PgPoolOptions;
use sqlx::types::BigDecimal;
use sqlx::{Pool, Postgres};
use std::error::Error;
use std::time::Duration;
use std::{env, fmt, process};
use tracing_subscriber::fmt::time::FormatTime;

// 1. Define a native custom error type
#[derive(Debug)]
pub enum AppError {
    Database(sqlx::Error),
    Config(String),
}

// 2. Implement standard Display formatting required by std::error::Error
impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Database(e) => write!(f, "Database failure: {}", e),
            AppError::Config(e) => write!(f, "Configuration error: {}", e),
        }
    }
}

// 3. Implement the core standard error trait
impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            AppError::Database(e) => Some(e),
            _ => None,
        }
    }
}

// 4. Implement standard From trait to support the native `?` operator automatically
impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        AppError::Database(err)
    }
}

#[derive(sqlx::FromRow, Debug)]
struct Candle {
    symbol: String,
    bucket_ist: chrono::NaiveDateTime,
    open: BigDecimal,
    high: BigDecimal,
    low: BigDecimal,
    close: BigDecimal,
    volume: i64,
}

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

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await?;

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

        if let Err(e) = fetch_latest_candles(&pool, &symbols).await {
            tracing::error!("Error pulling engine logs: {}", e);
            process::exit(1);
        }

        let post_fetch_time = Local::now().with_timezone(&Kolkata).time();
        if post_fetch_time >= market_close {
            tracing::info!("15:30 PM final interval met. Terminating app safely.");
            break;
        }
    }

    Ok(())
}

async fn fetch_latest_candles(pool: &Pool<Postgres>, symbols: &[String]) -> Result<(), AppError> {
    tracing::info!("Executing 5-minute interval database fetch...");

    let query = r#"
        SELECT symbol,
            time_bucket('5 minutes', ltt) AT TIME ZONE 'Asia/Kolkata' AS bucket_ist,
            first(ltp, ltt) AS open,
            max(ltp)        AS high,
            min(ltp)        AS low,
            last(ltp, ltt)  AS close,
            max(cum_volume) - min(cum_volume) AS volume
            FROM ticks
            WHERE symbol = ANY($1)
            GROUP BY symbol, time_bucket('5 minutes', ltt)
            ORDER BY bucket_ist DESC LIMIT 1;
        "#;

    // The ? operator natively maps via our From implementation
    let candles = sqlx::query_as::<_, Candle>(query)
        .bind(symbols)
        .fetch_all(pool)
        .await?;

    for candle in candles {
        tracing::info!(
            "[{}] Bucket IST: {}, O: {}, H: {}, L: {}, C: {}, Vol: {}",
            candle.symbol,
            candle.bucket_ist.format("%Y-%m-%d %H:%M:%S"),
            candle.open,
            candle.high,
            candle.low,
            candle.close,
            candle.volume
        );
    }

    Ok(())
}
