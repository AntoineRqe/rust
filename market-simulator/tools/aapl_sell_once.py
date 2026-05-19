#!/usr/bin/env python3
"""
Reads symbol list from market config files and sends SELL orders at live price fetched from NASDAQ.

Usage:
    python aapl_sell_once.py --password PASS [--gateway-url URL]
                             [--nasdaq-ws-url URL] [--nyse-ws-url URL]
                             [--username USER] [--qty QTY]
                             [--nasdaq-config PATH] [--nyse-config PATH]

Defaults connect to https://www.marketsim.site.
Requires: pip install websockets
"""
import argparse
import json
import re
import ssl
import asyncio
import time
import random
from typing import Dict, List
from urllib import request as urllib_request

try:
    import websockets
except ImportError:
    raise SystemExit("Missing dependency: pip install websockets")


# Global counter for unique cli_ord_id generation
_cli_ord_id_counter = 0
_cli_ord_id_lock = asyncio.Lock()


async def generate_unique_cli_ord_id(prefix: str) -> str:
    """Generate a unique cli_ord_id using timestamp and counter."""
    global _cli_ord_id_counter
    async with _cli_ord_id_lock:
        ts_ms = int(time.time() * 1000)
        _cli_ord_id_counter += 1
        # Format: PREFIX + timestamp + counter + random 3-digit suffix
        # e.g., SNA1779182348286001234 ensures uniqueness even with concurrent calls
        random_suffix = random.randint(0, 999)
        cli_ord_id = f"{prefix}{ts_ms}{_cli_ord_id_counter:03d}{random_suffix:03d}"
        return cli_ord_id


def load_market_config(config_path: str) -> Dict:
    """Load market configuration from JSON file."""
    try:
        with open(config_path, 'r') as f:
            return json.load(f)
    except Exception as e:
        raise RuntimeError(f"Failed to load market config from {config_path}: {e}")


def get_symbols_from_config(config_path: str) -> List[str]:
    """Extract symbol list from market config."""
    try:
        config = load_market_config(config_path)
        stocks = config.get("market", {}).get("stocks", [])
        return [s.upper() for s in stocks]
    except Exception as e:
        print(f"Warning: Could not read symbols from {config_path}: {e}")
        return []


async def discover_symbols(ws, timeout_s: float = 3.0) -> List[str]:
    """Dynamically discover available symbols from order_book messages."""
    symbols = set()
    start_time = asyncio.get_event_loop().time()
    
    try:
        while asyncio.get_event_loop().time() - start_time < timeout_s:
            try:
                raw = await asyncio.wait_for(ws.recv(), timeout=1.0)
                msg = json.loads(raw)
                
                if msg.get("type") == "order_book":
                    symbol = str(msg.get("symbol", "")).upper()
                    if symbol and symbol not in symbols:
                        symbols.add(symbol)
                        # Stop after discovering enough unique symbols
                        if len(symbols) >= 5:
                            return sorted(symbols)
            except asyncio.TimeoutError:
                if symbols:
                    return sorted(symbols)
    except Exception as e:
        pass
    
    return sorted(symbols) if symbols else []


def fetch_nasdaq_price(symbol: str) -> float:
    """Fetch the last AAPL price from NASDAQ API."""
    url = f"https://api.nasdaq.com/api/quote/{symbol}/info?assetclass=stocks"
    req = urllib_request.Request(
        url,
        headers={
            "User-Agent": "Mozilla/5.0 (X11; Linux x86_64)",
            "Accept": "application/json,text/plain,*/*",
            "Referer": f"https://www.nasdaq.com/market-activity/stocks/{symbol.lower()}",
            "Origin": "https://www.nasdaq.com",
        },
    )
    with urllib_request.urlopen(req, timeout=15) as resp:
        payload = json.loads(resp.read().decode("utf-8", errors="replace"))
    data = (payload or {}).get("data") or {}
    primary = data.get("primaryData") or {}
    candidate = primary.get("lastSalePrice") or primary.get("lastTrade") or data.get("lastSalePrice")
    if not candidate:
        raise RuntimeError(f"NASDAQ response missing last sale price: {payload}")
    match = re.search(r"-?\d+(?:\.\d+)?", str(candidate).replace(",", ""))
    if not match:
        raise RuntimeError(f"Cannot parse price from value: {candidate!r}")
    return float(match.group(0))

def gateway_login(gateway_url: str, username: str, password: str) -> Dict[str, str]:
    """POST /api/login to the gateway. Returns market -> token mapping."""
    login_url = gateway_url.rstrip("/") + "/api/login"
    data = json.dumps({"username": username, "password": password}).encode()
    req = urllib_request.Request(
        login_url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    ctx = ssl.create_default_context()
    with urllib_request.urlopen(req, timeout=10, context=ctx) as resp:
        body = json.loads(resp.read().decode())

    # Multi-market gateway response: { markets: [{name, token, url}, ...] }
    markets = body.get("markets")
    if markets:
        tokens: Dict[str, str] = {}
        for market in markets:
            name = str(market.get("name", "")).upper()
            token = market.get("token")
            if name and token:
                tokens[name] = token
        
        if tokens:
            print(f"Authenticated as '{username}' for markets: {', '.join(sorted(tokens.keys()))}")
            return tokens

    # Single-market fallback: { token: "..." }
    token = body.get("token")
    if not token:
        raise RuntimeError(f"Login failed: {body}")
    print(f"Authenticated as '{username}'")
    return {"NASDAQ": token, "NYSE": token}


async def send_sell_order(
    market_name: str,
    ws_url: str,
    token: str,
    username: str,
    symbol: str,
    qty: int,
    price: float,
) -> None:
    """Connect to market WebSocket and send a SELL order."""
    full_url = f"{ws_url}?token={token}&username={username}"
    
    ssl_ctx = ssl.create_default_context() if ws_url.startswith("wss://") else None
    
    async with websockets.connect(full_url, ssl=ssl_ctx) as ws:
        # Send the SELL order
        market_tag = "".join(ch for ch in market_name.upper() if ch.isalnum())[:2] or "MK"
        cl_ord_id = await generate_unique_cli_ord_id(f"S{market_tag}")
        order_cmd = json.dumps({
            "action": "order",
            "clord_id": cl_ord_id,
            "symbol": symbol,
            "qty": float(qty),
            "price": round(price, 1),
            "side": "2",  # SELL
        })
        await ws.send(order_cmd)
        print(f"Sent SELL {qty} {symbol} @ {price:.4f}")
        
        # Wait for execution report
        try:
            async with asyncio.timeout(5):
                while True:
                    raw = await ws.recv()
                    msg = json.loads(raw)
                    if msg.get("type") == "fix_message":
                        body = msg.get("body", "")
                        label = msg.get("label", "")
                        print(f"<< {label}: {body[:120]}")
                        if cl_ord_id in body and ("35=8" in body or "SENT ▶" in label):
                            print("Order sent successfully!")
                            break
        except TimeoutError:
            print("(no execution report within 5s — order may still be processing)")


async def main_async(args: argparse.Namespace) -> None:
    """Load symbol list and send sell orders to both markets."""
    # Define standard config file locations
    nasdaq_config_paths = [
        args.nasdaq_config,
        "./crates/config/markets/nasdaq.json",
        "../config/markets/nasdaq.json",
        "crates/config/markets/nasdaq.json",
    ]
    nyse_config_paths = [
        args.nyse_config,
        "./crates/config/markets/nyse.json",
        "../config/markets/nyse.json",
        "crates/config/markets/nyse.json",
    ]
    
    # Try to load symbols from config files
    nasdaq_symbols = []
    nyse_symbols = []
    
    for config_path in nasdaq_config_paths:
        if config_path:
            nasdaq_symbols = get_symbols_from_config(config_path)
            if nasdaq_symbols:
                print(f"✓ Loaded NASDAQ symbols from {config_path}: {nasdaq_symbols}")
                break
    
    for config_path in nyse_config_paths:
        if config_path:
            nyse_symbols = get_symbols_from_config(config_path)
            if nyse_symbols:
                print(f"✓ Loaded NYSE symbols from {config_path}: {nyse_symbols}")
                break
    
    symbols = sorted(set(nasdaq_symbols + nyse_symbols))

    if not symbols:
        # Fallback to command-line symbol if no config loaded
        print("No symbols loaded from config files, using provided symbols")
        symbols = [args.symbol.upper()]

    # Login to get tokens
    print(f"\nLogging in via gateway {args.gateway_url}...")
    tokens = gateway_login(args.gateway_url, args.username, args.password)

    nasdaq_token = tokens.get("NASDAQ")
    nyse_token = tokens.get("NYSE")
    
    if not nasdaq_token or not nyse_token:
        raise RuntimeError("Missing market token(s) for NASDAQ/NYSE")

    # Discover symbols dynamically from market if very few symbols loaded
    if len(symbols) < 2:
        print("Discovering additional symbols from market...")
        full_url_nasdaq = f"{args.nasdaq_ws_url}?token={nasdaq_token}&username={args.username}"
        ssl_ctx = ssl.create_default_context() if args.nasdaq_ws_url.startswith("wss://") else None
        
        try:
            async with websockets.connect(full_url_nasdaq, ssl=ssl_ctx) as ws:
                discovered = await discover_symbols(ws, timeout_s=3.0)
                if discovered:
                    print(f"✓ Discovered symbols from market: {', '.join(discovered)}")
                    symbols = sorted(set(symbols + discovered))
        except Exception as e:
            print(f"Could not discover symbols dynamically: {e}")

    print(f"\nFetching live prices and sending SELL orders for: {', '.join(symbols)}\n")

    print(f"Fetching live prices and sending SELL orders for: {', '.join(symbols)}\n")
    
    tasks = []
    for symbol in symbols:
        try:
            nasdaq_price = fetch_nasdaq_price(symbol)
            print(f"[{symbol}] NASDAQ price: {nasdaq_price:.4f}")
            
            # Apply delta to NYSE price
            delta = nasdaq_price * (args.delta_percent / 100.0)
            nyse_price = nasdaq_price + delta
            print(f"[{symbol}] NYSE price: {nyse_price:.4f} (+{args.delta_percent}%)\n")
            
            # Create tasks for both markets
            tasks.append(
                send_sell_order(
                    market_name="NASDAQ",
                    ws_url=args.nasdaq_ws_url,
                    token=nasdaq_token,
                    username=args.username,
                    symbol=symbol,
                    qty=args.qty,
                    price=nasdaq_price,
                )
            )
            tasks.append(
                send_sell_order(
                    market_name="NYSE",
                    ws_url=args.nyse_ws_url,
                    token=nyse_token,
                    username=args.username,
                    symbol=symbol,
                    qty=args.qty,
                    price=nyse_price,
                )
            )
        except Exception as e:
            print(f"[{symbol}] Error fetching price: {e}\n")
            continue
    
    if tasks:
        await asyncio.gather(*tasks)
    else:
        print("No tasks to execute")


def main() -> None:
    parser = argparse.ArgumentParser(description="Send SELL orders for all configured symbols to both markets at live price")
    parser.add_argument("--gateway-url", default="https://www.marketsim.site",
                        help="Base URL of the gateway (default: https://www.marketsim.site)")
    parser.add_argument("--nasdaq-ws-url", default="wss://www.marketsim.site/nasdaq/ws",
                        help="WebSocket URL for NASDAQ (default: wss://www.marketsim.site/nasdaq/ws)")
    parser.add_argument("--nyse-ws-url", default="wss://www.marketsim.site/nyse/ws",
                        help="WebSocket URL for NYSE (default: wss://www.marketsim.site/nyse/ws)")
    parser.add_argument("--nasdaq-config", default=None,
                        help="Path to NASDAQ market config JSON (optional)")
    parser.add_argument("--nyse-config", default=None,
                        help="Path to NYSE market config JSON (optional)")
    parser.add_argument("--username", default="admin", help="Login username (default: admin)")
    parser.add_argument("--password", required=True, help="Login password")
    parser.add_argument("--symbol", default="AAPL", help="Fallback symbol if config not provided (default: AAPL)")
    parser.add_argument("--qty", type=int, default=10, help="Sell quantity (default: 10)")
    parser.add_argument("--delta-percent", type=float, default=0.01,
                        help="Percent delta applied to NYSE price vs NASDAQ (default: 0.01)")
    args = parser.parse_args()
    asyncio.run(main_async(args))


if __name__ == "__main__":
    main()

