#!/usr/bin/env python3
"""
Connects to both markets and submits one BUY order at the current lowest ask for AAPL.

Usage:
    python aapl_buy_once.py --password PASS [--gateway-url URL]
                            [--nasdaq-ws-url URL] [--nyse-ws-url URL]
                            [--username USER] [--symbol SYMBOL] [--qty QTY]

Defaults connect to https://www.marketsim.site.
Requires: pip install websockets
"""

import argparse
import asyncio
import json
import ssl
import time
from typing import Dict, Optional
from urllib import request as urllib_request

try:
    import websockets
except ImportError:
    raise SystemExit("Missing dependency: pip install websockets")


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
        ts_ms = int(time.time() * 1000)
        market_tag = "".join(ch for ch in market_name.upper() if ch.isalnum())[:2] or "MK"
        cl_ord_id = f"B{market_tag}{ts_ms}"

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
        print(f"[{market_name}] Sent BUY {qty} {symbol} @ {lowest_ask:.4f}")

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
    symbol = args.symbol.upper()
    print(f"Logging in via gateway {args.gateway_url}...")
    tokens = gateway_login(args.gateway_url, args.username, args.password)

    nasdaq_token = tokens.get("NASDAQ") or tokens.get("NYSE")
    nyse_token = tokens.get("NYSE") or tokens.get("NASDAQ")

    if not nasdaq_token or not nyse_token:
        raise RuntimeError("Missing market token(s) for NASDAQ/NYSE")

    print(f"\nPlacing BUY {args.qty} {symbol} at current lowest ask on both markets...")
    await asyncio.gather(
        buy_once_on_market(
            market_name="NASDAQ",
            ws_url=args.nasdaq_ws_url,
            token=nasdaq_token,
            username=args.username,
            symbol=symbol,
            qty=args.qty,
        ),
        buy_once_on_market(
            market_name="NYSE",
            ws_url=args.nyse_ws_url,
            token=nyse_token,
            username=args.username,
            symbol=symbol,
            qty=args.qty,
        ),
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Buy once on both markets at each market's lowest current ask"
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
    parser.add_argument("--username", default="admin", help="Login username (default: admin)")
    parser.add_argument("--password", required=True, help="Login password")
    parser.add_argument("--symbol", default="AAPL", help="Symbol to buy (default: AAPL)")
    parser.add_argument("--qty", type=int, default=1, help="Buy quantity (default: 1)")

    args = parser.parse_args()
    if args.qty <= 0:
        raise SystemExit("--qty must be > 0")

    asyncio.run(main_async(args))


if __name__ == "__main__":
    main()
