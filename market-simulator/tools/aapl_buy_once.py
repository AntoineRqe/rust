#!/usr/bin/env python3
"""
Reads symbol list from market config files and submits BUY orders at current lowest ask.

Usage:
    python aapl_buy_once.py --password PASS [--gateway-url URL]
                            [--nasdaq-ws-url URL] [--nyse-ws-url URL]
                            [--username USER] [--qty QTY]
                            [--nasdaq-config PATH] [--nyse-config PATH]

Defaults connect to https://www.marketsim.site.
Requires: pip install websockets
"""

import argparse
import asyncio
import json
import random
import ssl
import time
from typing import Dict, List, Optional
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
        # e.g., BNA1779182348286001234 ensures uniqueness even with concurrent calls
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


def ensure_min_price(price: float, min_price: float = 10.0) -> float:
    """Ensure price is at least min_price by multiplying if needed.
    
    Preserves decimal precision throughout calculation:
    - If price >= min_price: return as-is
    - If price < min_price: multiply by 10, then by 10 again if still too low
    - Ensures precision is kept during multiplication before any rounding
    """
    if price >= min_price:
        return price
    
    # Multiply by 10 to bring low prices up (e.g., 1.5 -> 15.0)
    adjusted = price * 10.0
    
    # If still below minimum, multiply by 10 again (e.g., 0.5 -> 5 -> 50)
    if adjusted < min_price:
        adjusted = adjusted * 10.0
    
    # If somehow still below (should not happen), clamp to minimum
    if adjusted < min_price:
        adjusted = min_price
    
    return adjusted


def get_symbols_from_config(config_path: str) -> List[str]:
    """Extract symbol list from market config."""
    try:
        config = load_market_config(config_path)
        stocks = config.get("market", {}).get("stocks", [])
        return [s.upper() for s in stocks]
    except Exception as e:
        print(f"Warning: Could not read symbols from {config_path}: {e}")
        return []


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

    token = body.get("token")
    if not token:
        raise RuntimeError(f"Login failed: {body}")

    print(f"Authenticated as '{username}' (single-market token)")
    return {"NASDAQ": token, "NYSE": token}


def _best_ask_price(asks: object) -> Optional[float]:
    """Return the lowest ask price with positive quantity from an order_book asks list."""
    if not isinstance(asks, list):
        return None

    candidates = []
    for row in asks:
        if not isinstance(row, dict):
            continue
        try:
            price = float(row.get("price"))
            qty = float(row.get("quantity", 0))
        except (TypeError, ValueError):
            continue

        if price > 0 and qty > 0:
            candidates.append(price)

    if not candidates:
        return None
    return min(candidates)


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


async def fetch_lowest_ask(ws, symbol: str, timeout_s: float = 5.0) -> float:
    """Wait for order_book messages and return the lowest ask for the symbol."""
    target_symbol = symbol.upper()

    async with asyncio.timeout(timeout_s):
        while True:
            raw = await ws.recv()
            msg = json.loads(raw)

            if msg.get("type") != "order_book":
                continue

            msg_symbol = str(msg.get("symbol", "")).upper()
            if msg_symbol and msg_symbol != target_symbol:
                continue

            price = _best_ask_price(msg.get("asks"))
            if price is not None:
                return price

    raise RuntimeError(f"Timed out waiting for order book asks for {target_symbol}")


async def buy_once_on_market(
    market_name: str,
    ws_url: str,
    token: str,
    username: str,
    symbol: str,
    qty: int,
) -> None:
    """Connect to a market, discover lowest ask, send one BUY order, then wait for ack."""
    full_url = f"{ws_url}?token={token}&username={username}"
    ssl_ctx = ssl.create_default_context() if ws_url.startswith("wss://") else None

    async with websockets.connect(full_url, ssl=ssl_ctx) as ws:
        try:
            lowest_ask = await fetch_lowest_ask(ws, symbol)
        except (RuntimeError, TimeoutError):
            print(f"[{market_name}] No ask liquidity for {symbol}; skipping")
            return
        
        # Ensure price is at least $10
        lowest_ask = ensure_min_price(lowest_ask)
        
        market_tag = "".join(ch for ch in market_name.upper() if ch.isalnum())[:2] or "MK"
        cl_ord_id = await generate_unique_cli_ord_id(f"B{market_tag}")

        order_cmd = json.dumps(
            {
                "action": "order",
                "clord_id": cl_ord_id,
                "symbol": symbol,
                "qty": float(qty),
                "price": round(lowest_ask, 4),
                "side": "1",  # BUY
            }
        )
        await ws.send(order_cmd)
        print(f"[{market_name}] Sent BUY {qty} {symbol} @ {lowest_ask}")

        try:
            async with asyncio.timeout(5):
                while True:
                    raw = await ws.recv()
                    msg = json.loads(raw)
                    if msg.get("type") != "fix_message":
                        continue

                    body = msg.get("body", "")
                    label = msg.get("label", "")
                    print(f"[{market_name}] << {label}: {body[:120]}")

                    if cl_ord_id in body and ("35=8" in body or "SENT ▶" in label):
                        print(f"[{market_name}] BUY order sent successfully")
                        break
        except TimeoutError:
            print(f"[{market_name}] (no execution report within 5s — order may still be processing)")


async def main_async(args: argparse.Namespace) -> None:
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
    
    print(f"Logging in via gateway {args.gateway_url}...")
    tokens = gateway_login(args.gateway_url, args.username, args.password)

    nasdaq_token = tokens.get("NASDAQ") or tokens.get("NYSE")
    nyse_token = tokens.get("NYSE") or tokens.get("NASDAQ")

    if not nasdaq_token or not nyse_token:
        raise RuntimeError("Missing market token(s) for NASDAQ/NYSE")

    # Discover symbols dynamically from market if very few symbols loaded
    if len(symbols) < 2:
        print("\nDiscovering additional symbols from market...")
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
    
    print(f"\nPlacing BUY {args.qty} for symbols: {', '.join(symbols)} on both markets...\n")
    
    tasks = []
    for symbol in symbols:
        tasks.append(
            buy_once_on_market(
                market_name="NASDAQ",
                ws_url=args.nasdaq_ws_url,
                token=nasdaq_token,
                username=args.username,
                symbol=symbol,
                qty=args.qty,
            )
        )
        tasks.append(
            buy_once_on_market(
                market_name="NYSE",
                ws_url=args.nyse_ws_url,
                token=nyse_token,
                username=args.username,
                symbol=symbol,
                qty=args.qty,
            )
        )
    
    await asyncio.gather(*tasks)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Place BUY orders for all configured symbols on both markets at current lowest ask"
    )
    parser.add_argument(
        "--gateway-url",
        default="https://www.marketsim.site",
        help="Base URL of the gateway (default: https://www.marketsim.site)",
    )
    parser.add_argument(
        "--nasdaq-ws-url",
        default="wss://www.marketsim.site/nasdaq/ws",
        help="WebSocket URL for NASDAQ (default: wss://www.marketsim.site/nasdaq/ws)",
    )
    parser.add_argument(
        "--nyse-ws-url",
        default="wss://www.marketsim.site/nyse/ws",
        help="WebSocket URL for NYSE (default: wss://www.marketsim.site/nyse/ws)",
    )
    parser.add_argument(
        "--nasdaq-config",
        default=None,
        help="Path to NASDAQ market config JSON (optional)",
    )
    parser.add_argument(
        "--nyse-config",
        default=None,
        help="Path to NYSE market config JSON (optional)",
    )
    parser.add_argument("--username", default="admin", help="Login username (default: admin)")
    parser.add_argument("--password", required=True, help="Login password")
    parser.add_argument("--symbol", default="AAPL", help="Fallback symbol if config not provided (default: AAPL)")
    parser.add_argument("--qty", type=int, default=1, help="Buy quantity (default: 1)")

    args = parser.parse_args()
    if args.qty <= 0:
        raise SystemExit("--qty must be > 0")

    asyncio.run(main_async(args))


if __name__ == "__main__":
    main()
