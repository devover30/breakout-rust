# Database Configuration
export DATABASE_URL=postgres://db_user:User_2026@localhost:5432/trading

# List of trading symbols to track
export FYERS_SYMBOLS=NSE:BSE-EQ,NSE:NIFTY50-INDEX,NSE:MIDCPNIFTY-INDEX

.PHONY: run build clean

run:
	@echo "Launching breakout-rust trading engine with active watchlists..."
	cargo run

build:
	cargo build --release

clean:
	cargo clean
