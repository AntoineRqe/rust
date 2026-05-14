#!/usr/bin/env python3
"""
Open-loop load test for the market WebSocket order path.

Auths as admin using MARKET_SIMULATOR_ADMIN_PWD, then submits a stream of
WebSocket order commands against a single market.

Requires:
  pip install websockets

Examples:
  MARKET_SIMULATOR_ADMIN_PWD=secret python tools/load_test.py --ws-url ws://127.0.0.1:9870/ws
  MARKET_SIMULATOR_ADMIN_PWD=secret python tools/load_test.py --rate 500 --duration 30 --side 2
"""

from __future__ import annotations

import argparse
import asyncio
import contextlib
import json
import os
import ssl
import time
from dataclasses import dataclass, field
from statistics import mean
from typing import Dict, Optional
from urllib import request as urllib_request
from urllib.parse import urlparse, urlunparse

try:
    import websockets
except ImportError as exc:  # pragma: no cover
    raise SystemExit("Missing dependency: pip install websockets") from exc


def _http_post_json(url: str, payload: dict, timeout: float = 10.0) -> dict:
    req = urllib_request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    ctx = ssl.create_default_context() if url.startswith("https://") else None
    with urllib_request.urlopen(req, timeout=timeout, context=ctx) as resp:
        return json.loads(resp.read().decode("utf-8"))


def login(login_url: str, username: str, password: str, market: Optional[str]) -> str:
    body = _http_post_json(login_url, {"username": username, "password": password})
    markets = body.get("markets")
    if isinstance(markets, list) and markets:
        if market:
            for item in markets:
                if str(item.get("name", "")).upper() == market.upper():
                    token = item.get("token")
                    if token:
                        return str(token)
        token = markets[0].get("token")
        if token:
            return str(token)

    token = body.get("token")
    if token:
        return str(token)

    raise RuntimeError(f"Login failed: {body}")


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    values = sorted(values)
    idx = int(round((len(values) - 1) * pct))
    return values[max(0, min(idx, len(values) - 1))]


def default_market_ws_url(market: str) -> str:
    market = market.upper()
    if market == "NYSE":
        return "ws://127.0.0.1:19885/ws"
    return "ws://127.0.0.1:19870/ws"


CL_ORD_ID_MAX_LEN = 20


@dataclass
class Stats:
    sent: int = 0
    acked: int = 0
    errors: int = 0
    latencies_ms: list[float] = field(default_factory=list)


async def run_load_test(args: argparse.Namespace) -> None:
    pwd = os.getenv(args.password_env)
    if not pwd:
        raise SystemExit(f"Missing env var {args.password_env}")

    token = login(args.login_url, args.username, pwd, args.market)

    parsed_ws = urlparse(args.ws_url)
    if parsed_ws.netloc.startswith("127.0.0.1:9860") or parsed_ws.netloc.startswith("localhost:9860"):
        parsed_ws = urlparse(default_market_ws_url(args.market))
    elif parsed_ws.path in ("", "/", "/ws"):
        parsed_ws = parsed_ws._replace(path=f"/{args.market.lower()}/ws")
    ws_url = f"{urlunparse(parsed_ws)}?token={token}&username={args.username}"
    ssl_ctx = ssl.create_default_context() if parsed_ws.scheme == "wss" else None

    stats = Stats()
    pending: Dict[str, float] = {}
    pending_lock = asyncio.Lock()
    stop_at = time.monotonic() + args.duration
    sequence = 0
    interval = 1.0 / args.rate if args.rate > 0 else 0.0

    async with websockets.connect(ws_url, ssl=ssl_ctx, max_size=2**23) as ws:
        async def receiver() -> None:
            nonlocal stats
            while True:
                raw = await ws.recv()
                try:
                    event = json.loads(raw)
                except json.JSONDecodeError:
                    continue

                if event.get("type") != "fix_message":
                    continue

                body = str(event.get("body", ""))
                if "35=8" not in body:
                    continue

                clord_id = None
                for part in body.split("│"):
                    part = part.strip()
                    if part.startswith("11="):
                        clord_id = part[3:]
                        break
                if clord_id:
                    clord_id = clord_id.split("\x00", 1)[0]
                if not clord_id:
                    continue

                async with pending_lock:
                    started = pending.pop(clord_id, None)

                if started is not None:
                    stats.acked += 1
                    stats.latencies_ms.append((time.perf_counter() - started) * 1000.0)

        recv_task = asyncio.create_task(receiver())
        try:
            next_send = time.monotonic()
            while time.monotonic() < stop_at:
                now = time.monotonic()
                if now < next_send:
                    await asyncio.sleep(next_send - now)
                next_send += interval

                sequence += 1
                clord_id = f"LT{sequence:018d}"
                if len(clord_id) > CL_ORD_ID_MAX_LEN:
                    raise RuntimeError(f"clord_id exceeds FIX width {CL_ORD_ID_MAX_LEN}: {clord_id}")
                payload = {
                    "action": "order",
                    "clord_id": clord_id,
                    "symbol": args.symbol,
                    "qty": float(args.qty),
                    "price": float(args.price),
                    "side": args.side,
                    "sender": None,
                    "target": None,
                }

                async with pending_lock:
                    pending[clord_id] = time.perf_counter()
                await ws.send(json.dumps(payload))
                stats.sent += 1

            if not args.skip_clear_book:
                await ws.send(json.dumps({"action": "clear_book"}))

            await asyncio.sleep(args.settle)
        finally:
            recv_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await recv_task

    elapsed = args.duration
    sent_rps = stats.sent / elapsed if elapsed else 0.0
    ack_rps = stats.acked / elapsed if elapsed else 0.0
    avg = mean(stats.latencies_ms) if stats.latencies_ms else 0.0

    print(f"sent={stats.sent} acked={stats.acked} errors={stats.errors}")
    print(f"throughput_sent_rps={sent_rps:.2f} throughput_acked_rps={ack_rps:.2f}")
    if stats.latencies_ms:
        print(
            "latency_ms "
            f"avg={avg:.2f} p50={percentile(stats.latencies_ms, 0.50):.2f} "
            f"p95={percentile(stats.latencies_ms, 0.95):.2f} "
            f"p99={percentile(stats.latencies_ms, 0.99):.2f}"
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Load test the market WebSocket order path")
    parser.add_argument("--login-url", default="http://127.0.0.1:9860/api/login")
    parser.add_argument("--ws-url", default="ws://127.0.0.1:19870/ws")
    parser.add_argument("--username", default="admin")
    parser.add_argument("--password-env", default="MARKET_SIMULATOR_ADMIN_PWD")
    parser.add_argument("--market", default="NASDAQ")
    parser.add_argument("--symbol", default="AAPL")
    parser.add_argument("--side", choices=["1", "2"], default="2", help='FIX side code: "1" buy, "2" sell')
    parser.add_argument("--qty", type=float, default=1.0)
    parser.add_argument("--price", type=float, default=100.0)
    parser.add_argument("--rate", type=float, default=100.0, help="Target orders/sec")
    parser.add_argument("--duration", type=float, default=30.0)
    parser.add_argument("--settle", type=float, default=2.0, help="Seconds to wait for late acks")
    parser.add_argument("--skip-clear-book", action="store_true", help="Do not send clear_book at end")
    return parser.parse_args()


if __name__ == "__main__":
    asyncio.run(run_load_test(parse_args()))
