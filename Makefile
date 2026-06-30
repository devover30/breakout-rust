# Database Configuration
export DATABASE_URL=postgres://db_user:User_2026@localhost:5432/trading
export REDIS_URL=redis://127.0.0.1:6379/0
# List of trading symbols to track
export FYERS_SYMBOLS=NSE:NIFTY50-INDEX,NSE:MIDCPNIFTY-INDEX

.PHONY: run build clean

run:
	@echo "Launching breakout-rust trading engine with active watchlists..."
	cargo run

build:
	cargo build --release

clean:
	cargo clean
