#!/usr/bin/env python3
"""
Fetches live AAPL price from NASDAQ, then places (or refreshes) a SELL limit
order on each market via the web-backend WebSocket API.

Orders flow through the backend FIX session manager, so execution reports
update the order book displayed in the browser.

Usage:
    python aapl_sell_once.py [--nasdaq-url URL] [--nyse-url URL]
                             [--username USER] [--password PASS]
                             [--qty QTY] [--delta-percent DELTA]
                             [--dry-run]

Defaults connect to https://www.marketsim.site (path-based routing).
Requires: pip install websockets
"""
import argparse
import json
import re
import ssl
import asyncio
from urllib import request as urllib_request

try:
    import websockets
except ImportError:
    raise SystemExit("Missing dependency: pip install websockets")

# ── price fetch ───────────────────────────────────────────────────────────────

def fetch_nasdaq_price(symbol: str) -> float:
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

# ── login ─────────────────────────────────────────────────────────────────────

def gateway_login(gateway_url: str, username: str, password: str) -> dict:
    """POST /api/login to the gateway. Returns dict of market_name -> token."""
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
        result = {m["name"].upper(): m["token"] for m in markets}
        print(f"  Authenticated as '{username}', got tokens for: {list(result.keys())}")
        return result

    # Single-market fallback: { token: "..." }
    token = body.get("token")
    if not token:
        raise RuntimeError(f"Login failed: {body}")
    print(f"  Authenticated as '{username}' (single-market token)")
    return {"__single__": token}

# ── WebSocket order placement ─────────────────────────────────────────────────

async def place_orders_via_ws(
    ws_url: str,
    token: str,
    username: str,
    cl_ord_id: str,
    symbol: str,
    qty: int,
    price: float,
    dry_run: bool,
) -> None:
    """Connect to the market WS, cancel any previous order with this cl_ord_id,
    then place a fresh SELL limit order. Waits for the execution report."""
    full_url = f"{ws_url}?token={token}&username={username}&market={_market_from_clordid(cl_ord_id)}"

    if dry_run:
        print(f"  [DRY RUN] Would connect to {ws_url} and SELL {qty} {symbol} @ {price:.4f} (clordid={cl_ord_id})")
        return

    ssl_ctx = ssl.create_default_context() if ws_url.startswith("wss://") else None

    async with websockets.connect(full_url, ssl=ssl_ctx) as ws:
        # Cancel the previous order with the same cl_ord_id (idempotent if absent)
        cancel_cmd = json.dumps({
            "action": "cancel",
            "clord_id": cl_ord_id,
            "symbol": symbol,
            "qty": float(qty),
        })
        await ws.send(cancel_cmd)
        print(f"  Sent CANCEL {cl_ord_id}")

        # Brief pause so the cancel reaches the engine before the new order
        await asyncio.sleep(0.1)

        # Place the new SELL limit order
        order_cmd = json.dumps({
            "action": "order",
            "clord_id": cl_ord_id,
            "symbol": symbol,
            "qty": float(qty),
            "price": round(price, 4),
            "side": "2",  # SELL
        })
        await ws.send(order_cmd)
        print(f"  Sent SELL {qty} {symbol} @ {price:.4f} (clordid={cl_ord_id})")

        # Wait for the execution report (39=0 New) or any ack, timeout 5s
        try:
            async with asyncio.timeout(5):
                while True:
                    raw = await ws.recv()
                    msg = json.loads(raw)
                    if msg.get("type") == "fix_message":
                        body = msg.get("body", "")
                        label = msg.get("label", "")
                        print(f"  << {label}: {body[:120]}")
                        # Stop on execution report (35=8) for our order, or on the
                        # backend's own "SENT ▶" ack.  Exclude cancel rejects (35=9)
                        # which also carry orig_cl_ord_id in the body.
                        if cl_ord_id in body and ("35=8" in body or "SENT \u25b6" in label):
                            break
        except TimeoutError:
            print("  (no execution report within 5 s — order may still be processing)")


def _market_from_clordid(cl_ord_id: str) -> str:
    """Extract market name from cl_ord_id like 'AUTOSELL-NASDAQ'."""
    parts = cl_ord_id.upper().split("-")
    return parts[-1] if len(parts) > 1 else "NASDAQ"

# ── main ──────────────────────────────────────────────────────────────────────

async def async_main(args: argparse.Namespace) -> None:
    nasdaq_ws = args.nasdaq_ws_url
    nyse_ws   = args.nyse_ws_url

    # Fetch live price
    price = fetch_nasdaq_price("AAPL")
    print(f"AAPL NASDAQ price (official): {price:.4f}")

    delta = price * (args.delta_percent / 100.0)
    nyse_price = price + delta

    # Login once via the gateway to get per-market tokens
    print(f"\nLogging in via gateway {args.gateway_url}...")
    tokens = gateway_login(args.gateway_url, args.username, args.password)

    nasdaq_token = tokens.get("NASDAQ") or tokens.get("__single__")
    nyse_token   = tokens.get("NYSE")   or tokens.get("__single__")

    if not nasdaq_token:
        raise RuntimeError("No NASDAQ token in gateway login response")
    if not nyse_token:
        raise RuntimeError("No NYSE token in gateway login response")

    # Place orders
    print(f"\n[NASDAQ] Placing SELL {args.qty} AAPL @ {price:.4f}...")
    await place_orders_via_ws(
        ws_url=nasdaq_ws,
        token=nasdaq_token,
        username=args.username,
        cl_ord_id="AUTOSELL-NASDAQ",
        symbol="AAPL",
        qty=args.qty,
        price=price,
        dry_run=args.dry_run,
    )

    print(f"\n[NYSE] Placing SELL {args.qty} AAPL @ {nyse_price:.4f} (+{args.delta_percent}%)...")
    await place_orders_via_ws(
        ws_url=nyse_ws,
        token=nyse_token,
        username=args.username,
        cl_ord_id="AUTOSELL-NYSE",
        symbol="AAPL",
        qty=args.qty,
        price=nyse_price,
        dry_run=args.dry_run,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="AAPL sell order bot — routes via backend WebSocket")
    parser.add_argument("--gateway-url", default="https://www.marketsim.site",
                        help="Base URL of the gateway (login) server")
    parser.add_argument("--nasdaq-ws-url", default="wss://www.marketsim.site/nasdaq/ws",
                        help="WebSocket URL for the NASDAQ market")
    parser.add_argument("--nyse-ws-url", default="wss://www.marketsim.site/nyse/ws",
                        help="WebSocket URL for the NYSE market")
    parser.add_argument("--username", default="admin", help="Login username")
    parser.add_argument("--password", required=True , help="Login password")
    parser.add_argument("--delta-percent", type=float, default=0.1,
                        help="Percent delta applied to NYSE price vs NASDAQ (default: 0.1)")
    parser.add_argument("--qty", type=int, default=10, help="Sell quantity (default: 10)")
    parser.add_argument("--dry-run", action="store_true", help="Print actions without sending")
    args = parser.parse_args()
    asyncio.run(async_main(args))


if __name__ == "__main__":
    main()
