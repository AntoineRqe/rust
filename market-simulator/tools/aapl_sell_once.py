#!/usr/bin/env python3
"""
Fetches live AAPL price from NASDAQ and sends a single SELL order via WebSocket.

Usage:
    python aapl_sell_once.py --password PASS [--gateway-url URL] [--ws-url URL]
                             [--username USER] [--qty QTY]

Defaults connect to https://www.marketsim.site.
Requires: pip install websockets
"""
import argparse
import json
import re
import ssl
import asyncio
import time
from urllib import request as urllib_request

try:
    import websockets
except ImportError:
    raise SystemExit("Missing dependency: pip install websockets")


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

def gateway_login(gateway_url: str, username: str, password: str) -> str:
    """POST /api/login to the gateway. Returns a token."""
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
        # Return NASDAQ token if available
        for m in markets:
            if m["name"].upper() == "NASDAQ":
                print(f"Authenticated as '{username}'")
                return m["token"]
        # Fallback to first market
        print(f"Authenticated as '{username}'")
        return markets[0]["token"]

    # Single-market fallback: { token: "..." }
    token = body.get("token")
    if not token:
        raise RuntimeError(f"Login failed: {body}")
    print(f"Authenticated as '{username}'")
    return token


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
        ts_ms = int(time.time() * 1000)
        market_tag = "".join(ch for ch in market_name.upper() if ch.isalnum())[:2] or "MK"
        cl_ord_id = f"S{market_tag}{ts_ms}"
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
    """Fetch AAPL price and send sell orders to both markets."""
    # Fetch live price
    nasdaq_price = fetch_nasdaq_price("AAPL")
    print(f"AAPL NASDAQ price: {nasdaq_price:.4f}")
    
    # Apply delta to NYSE price
    delta = nasdaq_price * (args.delta_percent / 100.0)
    nyse_price = nasdaq_price + delta
    print(f"AAPL NYSE price: {nyse_price:.4f} (+{args.delta_percent}%)\n")

    # Login to get token
    print(f"Logging in via gateway {args.gateway_url}...")
    token = gateway_login(args.gateway_url, args.username, args.password)

    # Send the SELL orders to both markets concurrently
    print(f"\nSending SELL orders...")
    await asyncio.gather(
        send_sell_order(
            market_name="NASDAQ",
            ws_url=args.nasdaq_ws_url,
            token=token,
            username=args.username,
            symbol="AAPL",
            qty=args.qty,
            price=nasdaq_price,
        ),
        send_sell_order(
            market_name="NYSE",
            ws_url=args.nyse_ws_url,
            token=token,
            username=args.username,
            symbol="AAPL",
            qty=args.qty,
            price=nyse_price,
        ),
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Send AAPL sell orders to both markets at last NASDAQ price")
    parser.add_argument("--gateway-url", default="https://www.marketsim.site",
                        help="Base URL of the gateway (default: https://www.marketsim.site)")
    parser.add_argument("--nasdaq-ws-url", default="wss://www.marketsim.site/nasdaq/ws",
                        help="WebSocket URL for NASDAQ (default: wss://www.marketsim.site/nasdaq/ws)")
    parser.add_argument("--nyse-ws-url", default="wss://www.marketsim.site/nyse/ws",
                        help="WebSocket URL for NYSE (default: wss://www.marketsim.site/nyse/ws)")
    parser.add_argument("--username", default="admin", help="Login username (default: admin)")
    parser.add_argument("--password", required=True, help="Login password")
    parser.add_argument("--qty", type=int, default=10, help="Sell quantity (default: 10)")
    parser.add_argument("--delta-percent", type=float, default=0.01,
                        help="Percent delta applied to NYSE price vs NASDAQ (default: 0.01)")
    args = parser.parse_args()
    asyncio.run(main_async(args))


if __name__ == "__main__":
    main()

