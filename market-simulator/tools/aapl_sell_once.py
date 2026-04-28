#!/usr/bin/env python3
"""
Fetches live AAPL price from NASDAQ (via TradeWatch),
submits a SELL order for 100 AAPL to the NASDAQ market simulator at this price,
and submits a SELL order for 100 AAPL to the NYSE market simulator at +/- a percentage of this price.

Usage:
    python aapl_sell_once.py [--config CONFIG] [--nasdaq-market-name NASDAQ] [--nyse-market-name NYSE] [--delta-percent DELTA]

- Requires TradeWatch API key in TRADEWATCH_API_KEY env variable.
- Market endpoints are loaded from the config JSON (default: crates/config/default.json).
"""
import os
import argparse
import json
import socket
from pathlib import Path
from typing import Dict, Any

def fetch_nasdaq_price(symbol: str) -> float:
    import re, json
    from urllib import request
    url = f"https://api.nasdaq.com/api/quote/{symbol}/info?assetclass=stocks"
    req = request.Request(
        url,
        headers={
            "User-Agent": "Mozilla/5.0 (X11; Linux x86_64)",
            "Accept": "application/json,text/plain,*/*",
            "Referer": f"https://www.nasdaq.com/market-activity/stocks/{symbol.lower()}",
            "Origin": "https://www.nasdaq.com",
        },
    )
    with request.urlopen(req, timeout=15) as resp:
        body = resp.read().decode("utf-8", errors="replace")
        payload = json.loads(body)
    data = (payload or {}).get("data") or {}
    primary = data.get("primaryData") or {}
    candidate = primary.get("lastSalePrice") or primary.get("lastTrade") or data.get("lastSalePrice")
    if not candidate:
        raise RuntimeError(f"NASDAQ response missing last sale price: {payload}")
    match = re.search(r"-?\d+(?:\.\d+)?", str(candidate).replace(",", ""))
    if not match:
        raise RuntimeError(f"Cannot parse price from value: {candidate!r}")
    return float(match.group(0))

DEFAULT_CONFIG = Path(__file__).resolve().parents[1] / "crates" / "config" / "default.json"
SOH = "\x01"

class MarketEndpoint:
    def __init__(self, name: str, ip: str, port: int):
        self.name = name
        self.ip = ip
        self.port = port

def fld(tag: int, val: Any) -> str:
    return f"{tag}={val}{SOH}"

def checksum(raw: str) -> str:
    return str(sum(ord(c) for c in raw) % 256).zfill(3)

def wrap_fix(body: str) -> bytes:
    begin = fld(8, "FIX.4.2")
    blen = fld(9, len(begin) + len(body))
    raw = begin + blen + body
    raw += fld(10, checksum(raw))
    return raw.encode("ascii")

def build_sell_limit_order(sender, target, symbol, qty, price, seq, cl_ord_id) -> bytes:
    import time
    from datetime import datetime, timezone
    now = datetime.now(timezone.utc).strftime("%Y%m%d-%H:%M:%S")
    body = (
        fld(35, "D")
        + fld(49, sender)
        + fld(56, target)
        + fld(34, seq)
        + fld(52, now)
        + fld(11, cl_ord_id)
        + fld(21, 1)
        + fld(55, symbol)
        + fld(54, 2)
        + fld(60, now)
        + fld(38, int(qty))
        + fld(40, 2)
        + fld(44, f"{price:.4f}")
    )
    return wrap_fix(body)

def load_market_endpoints(config_path: Path) -> Dict[str, MarketEndpoint]:
    with open(config_path, "r", encoding="utf-8") as f:
        cfg = json.load(f)
    out = {}
    for market in cfg.get("markets", []):
        name = str(market.get("name", "")).strip().upper()
        tcp = market.get("tcp") or {}
        ip = str(tcp.get("ip", "127.0.0.1"))
        port = int(tcp.get("port", 0))
        if name and port > 0:
            out[name] = MarketEndpoint(name, ip, port)
    return out

def send_fix_message(endpoint: MarketEndpoint, msg: bytes) -> None:
    with socket.create_connection((endpoint.ip, endpoint.port), timeout=8) as sock:
        sock.sendall(msg)
        sock.settimeout(0.2)
        try:
            _ = sock.recv(4096)
        except OSError:
            pass

def main():
    parser = argparse.ArgumentParser(description="AAPL sell order bot (single run)")
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG, help="Path to config JSON")
    parser.add_argument("--nasdaq-market-name", default="NASDAQ", help="Market name for NASDAQ in config")
    parser.add_argument("--nyse-market-name", default="NYSE", help="Market name for NYSE in config")
    parser.add_argument("--delta-percent", type=float, default=0.1, help="Percent delta for NYSE price (default: 0.1)")
    parser.add_argument("--qty", type=int, default=100, help="Sell quantity")
    parser.add_argument("--sender", default="AUTO_BOT", help="FIX SenderCompID (49)")
    parser.add_argument("--target", default="SERVER1", help="FIX TargetCompID (56)")
    parser.add_argument("--dry-run", action="store_true", help="Print actions, do not send orders")
    args = parser.parse_args()

    endpoints = load_market_endpoints(args.config)
    nasdaq_name = args.nasdaq_market_name.upper()
    nyse_name = args.nyse_market_name.upper()
    for name in (nasdaq_name, nyse_name):
        if name not in endpoints:
            raise RuntimeError(f"Market '{name}' not found in config endpoints")

    # Fetch price from official NASDAQ API
    price = fetch_nasdaq_price("AAPL")
    print(f"AAPL NASDAQ price (official): {price:.4f}")

    # NASDAQ order
    seq = 1
    cl_ord_id_nasdaq = f"AUTOSELL-NASDAQ"
    msg_nasdaq = build_sell_limit_order(
        sender=args.sender,
        target=args.target,
        symbol="AAPL",
        qty=args.qty,
        price=price,
        seq=seq,
        cl_ord_id=cl_ord_id_nasdaq,
    )
    if args.dry_run:
        print(f"[DRY RUN] Would send SELL {args.qty} AAPL @ {price:.4f} to {nasdaq_name}")
    else:
        send_fix_message(endpoints[nasdaq_name], msg_nasdaq)
        print(f"Sent SELL {args.qty} AAPL @ {price:.4f} to {nasdaq_name}")

    # NYSE order at +/- delta percent
    seq += 1
    delta = price * (args.delta_percent / 100.0)
    nyse_price = price + delta
    cl_ord_id_nyse = f"AUTOSELL-NYSE"
    msg_nyse = build_sell_limit_order(
        sender=args.sender,
        target=args.target,
        symbol="AAPL",
        qty=args.qty,
        price=nyse_price,
        seq=seq,
        cl_ord_id=cl_ord_id_nyse,
    )
    if args.dry_run:
        print(f"[DRY RUN] Would send SELL {args.qty} AAPL @ {nyse_price:.4f} to {nyse_name}")
    else:
        send_fix_message(endpoints[nyse_name], msg_nyse)
        print(f"Sent SELL {args.qty} AAPL @ {nyse_price:.4f} to {nyse_name}")

if __name__ == "__main__":
    main()
