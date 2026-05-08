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
import time
from typing import Optional
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
    cancel_last: bool,
    cancel_only: bool,
    cancel_clord_id: Optional[str],
    repeat_every: float,
) -> None:
    """Connect to the market WS, optionally BUY against an existing pending order, then
    optionally place a fresh SELL limit order. Waits for the execution report."""
    full_url = f"{ws_url}?token={token}&username={username}&market={_market_from_clordid(cl_ord_id)}"

    if dry_run:
        print(f"  [DRY RUN] Would connect to {ws_url}")
        if cancel_clord_id:
            print(f"  [DRY RUN] Would BUY against {cancel_clord_id}")
        elif cancel_last:
            print(f"  [DRY RUN] Would BUY against last pending SELL for {symbol}")
        else:
            print(f"  [DRY RUN] Would BUY against {cl_ord_id}")
        if not cancel_only:
            print(f"  [DRY RUN] Would SELL {qty} {symbol} @ {price:.4f} (clordid={cl_ord_id})")
        return

    ssl_ctx = ssl.create_default_context() if ws_url.startswith("wss://") else None

    async with websockets.connect(full_url, ssl=ssl_ctx) as ws:
        cycle = 0
        while True:
            cycle += 1
            if repeat_every > 0:
                print(f"  -- cycle {cycle} on {_market_from_clordid(cl_ord_id)} --")

            # Discover pending orders from player_state messages.
            pending_orders = []
            try:
                async with asyncio.timeout(2):
                    while True:
                        raw = await ws.recv()
                        msg = json.loads(raw)
                        if msg.get("type") == "player_state":
                            pending_orders = msg.get("pending_orders") or []
                            break
            except TimeoutError:
                pass

            target_id = cl_ord_id
            target_qty = float(qty)
            target_symbol = symbol
            target_price = price
            target_order = None

            if cancel_clord_id:
                target_id = cancel_clord_id
            elif cancel_last and pending_orders:
                # Pick the newest pending SELL order for this symbol; fallback to newest any-side.
                symbol_sell = [
                    o for o in pending_orders
                    if str(o.get("symbol", "")).upper() == symbol.upper() and str(o.get("side", "")) == "2"
                ]
                same_symbol = [
                    o for o in pending_orders
                    if str(o.get("symbol", "")).upper() == symbol.upper()
                ]
                pool = symbol_sell or same_symbol or pending_orders
                target_order = pool[-1]
                target_id = str(target_order.get("cl_ord_id") or cl_ord_id)

            # Resolve qty/symbol/price for the target order if available.
            if target_order is None and pending_orders:
                for order in pending_orders:
                    if str(order.get("cl_ord_id", "")) == target_id:
                        target_order = order
                        break

            if target_order is not None:
                target_qty = float(target_order.get("qty") or qty)
                target_symbol = str(target_order.get("symbol") or symbol)
                target_price = float(target_order.get("price") or price)

            # Place BUY order to offset the targeted pending SELL order.
            buy_cl_ord_id = f"AUTOBUY-{_market_from_clordid(cl_ord_id)}-{int(time.time() * 1000)}"
            buy_cmd = json.dumps({
                "action": "order",
                "clord_id": buy_cl_ord_id,
                "symbol": target_symbol,
                "qty": target_qty,
                "price": round(target_price, 4),
                "side": "1",  # BUY
            })
            await ws.send(buy_cmd)
            print(
                f"  Sent BUY {target_qty} {target_symbol} @ {target_price:.4f} "
                f"(target={target_id}, clordid={buy_cl_ord_id})"
            )

            # Wait for BUY ack / exec report so follow-up SELL runs after the offset attempt.
            try:
                async with asyncio.timeout(5):
                    while True:
                        raw = await ws.recv()
                        msg = json.loads(raw)
                        if msg.get("type") == "fix_message":
                            body = msg.get("body", "")
                            label = msg.get("label", "")
                            print(f"  << {label}: {body[:120]}")
                            if buy_cl_ord_id in body and ("35=8" in body or "SENT \u25b6" in label):
                                break
            except TimeoutError:
                print("  (no BUY execution report within 5 s — order may still be processing)")

            if not cancel_only:
                # Brief pause so the BUY reaches the engine before the new SELL.
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

            if repeat_every <= 0:
                break

            await asyncio.sleep(repeat_every)


def _market_from_clordid(cl_ord_id: str) -> str:
    """Extract market name from cl_ord_id like 'AUTOSELL-NASDAQ'."""
    parts = cl_ord_id.upper().split("-")
    return parts[-1] if len(parts) > 1 else "NASDAQ"

# ── main ──────────────────────────────────────────────────────────────────────

async def async_main(args: argparse.Namespace) -> None:
    nasdaq_ws = args.nasdaq_ws_url
    nyse_ws   = args.nyse_ws_url

    price = 0.0
    nyse_price = 0.0
    if not args.cancel_only:
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

    if args.repeat_every > 0:
        print(f"\nPersistent mode enabled: reusing WS sessions, cycle every {args.repeat_every}s")
        await asyncio.gather(
            place_orders_via_ws(
                ws_url=nasdaq_ws,
                token=nasdaq_token,
                username=args.username,
                cl_ord_id="AUTOSELL-NASDAQ",
                symbol="AAPL",
                qty=args.qty,
                price=price,
                dry_run=args.dry_run,
                cancel_last=args.cancel_last,
                cancel_only=args.cancel_only,
                cancel_clord_id=args.cancel_clord_id,
                repeat_every=args.repeat_every,
            ),
            place_orders_via_ws(
                ws_url=nyse_ws,
                token=nyse_token,
                username=args.username,
                cl_ord_id="AUTOSELL-NYSE",
                symbol="AAPL",
                qty=args.qty,
                price=nyse_price,
                dry_run=args.dry_run,
                cancel_last=args.cancel_last,
                cancel_only=args.cancel_only,
                cancel_clord_id=args.cancel_clord_id,
                repeat_every=args.repeat_every,
            ),
        )
    else:
        # Place/cancel orders once per market
        if args.cancel_only:
            print("\n[NASDAQ] Sending BUY request...")
        else:
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
            cancel_last=args.cancel_last,
            cancel_only=args.cancel_only,
            cancel_clord_id=args.cancel_clord_id,
            repeat_every=0,
        )

        if args.cancel_only:
            print("\n[NYSE] Sending BUY request...")
        else:
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
            cancel_last=args.cancel_last,
            cancel_only=args.cancel_only,
            cancel_clord_id=args.cancel_clord_id,
            repeat_every=0,
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
    parser.add_argument("--cancel-last", action="store_true",
                        help="Target the most recent pending order for symbol when preparing the BUY offset")
    parser.add_argument("--cancel-only", action="store_true",
                        help="Only send BUY request(s); do not place new SELL orders")
    parser.add_argument("--cancel-clord-id", default=None,
                        help="Explicit target clOrdID for BUY offset (overrides --cancel-last)")
    parser.add_argument("--repeat-every", type=float, default=0.0,
                        help="Keep WS sessions open and repeat cancel/order cycle every N seconds")
    parser.add_argument("--dry-run", action="store_true", help="Print actions without sending")
    args = parser.parse_args()
    asyncio.run(async_main(args))


if __name__ == "__main__":
    main()
