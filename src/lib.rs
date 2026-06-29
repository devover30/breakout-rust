use sqlx::postgres::PgPoolOptions;
use sqlx::types::BigDecimal;
use sqlx::{Pool, Postgres};
use std::error::Error;
use std::fmt;
use tokio::task::JoinSet;

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
pub struct Candle {
    pub symbol: String,
    pub bucket_ist: chrono::NaiveDateTime,
    pub open: BigDecimal,
    pub high: BigDecimal,
    pub low: BigDecimal,
    pub close: BigDecimal,
    pub volume: i64,
}

// =========================================================================
// The Stateful Engine Wrapper (Hides SQLx from the outside world)
// =========================================================================
pub struct PivotEngine {
    // Private field: External projects cannot see or interact with this Pool directly
    pool: Pool<Postgres>,
}

impl PivotEngine {
    /// Initializes the engine and handles the DB connection internally

    pub async fn new(db_url: &str, max_connections: u32) -> Result<Self, AppError> {
        tracing::info!("Library initializing private database connection pool...");

        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(db_url)
            .await?;

        tracing::info!("Private database connection pool established.");
        Ok(Self { pool })
    }

    /// Fetches the latest candles using the internal private connection pool
    pub async fn fetch_latest_candles(&self, symbols: &[String]) -> Result<Vec<Candle>, AppError> {
        tracing::info!("Querying metrics concurrently across the watchlist...");

        let mut set = JoinSet::new();
        let mut fetched_candles = Vec::new();

        // 1. Spawn each query as an concurrent task into the JoinSet
        for symbol in symbols {
            let pool = self.pool.clone();
            let symbol = symbol.clone();

            set.spawn(async move {
                let query = r#"
                SELECT symbol,
                time_bucket('5 minutes', ltt) AT TIME ZONE 'Asia/Kolkata' AS bucket_ist,
                first(ltp, ltt) AS open,
                max(ltp)        AS high,
                min(ltp)        AS low,
                last(ltp, ltt)  AS close,
                max(cum_volume) - min(cum_volume) AS volume
                FROM ticks
                WHERE symbol = $1
                GROUP BY symbol, time_bucket('5 minutes', ltt)
                ORDER BY bucket_ist DESC 
                LIMIT 1;
            "#;

                sqlx::query_as::<_, Candle>(query)
                    .bind(symbol)
                    .fetch_optional(&pool)
                    .await
            });
        }

        // 2. Await the tasks as they complete
        while let Some(res) = set.join_next().await {
            match res {
                // The outer Result handles task spawning panics
                Ok(db_res) => {
                    // The inner Result handles actual SQLx database execution errors
                    match db_res {
                        Ok(Some(candle)) => fetched_candles.push(candle),
                        Ok(None) => {} // No records found for this symbol yet
                        Err(e) => tracing::error!("Database query execution failure: {}", e),
                    }
                }
                Err(join_err) => tracing::error!("Task join execution failed: {}", join_err),
            }
        }

        Ok(fetched_candles)
    }
}
