"""
FIX 4.2 Web Terminal
====================
A self-contained Python HTTP server that:
  - Serves a single-page trading GUI at http://localhost:8080
  - Manages a persistent TCP connection to the FIX server
  - Pushes real-time updates to the browser via Server-Sent Events (SSE)
  - Accepts order/config commands from the browser via POST /api/action

No third-party packages required — stdlib only.

Run:  python fix_web_terminal.py [--http-port 8080] [--fix-host 127.0.0.1] [--fix-port 9876]
Then open:  http://localhost:8080
"""

import argparse
import base64
import hashlib
import json
import os
import secrets
import socket
import ssl
import threading
import time
import queue
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

# ─────────────────────────────────────────────────────────────────────────────
#  FIX helpers  (identical to the tkinter version)
# ─────────────────────────────────────────────────────────────────────────────
SOH  = "\x01"
_seq = 0
_seq_lock = threading.Lock()

def next_seq():
    global _seq
    with _seq_lock:
        _seq += 1
        return _seq

def reset_seq():
    global _seq
    with _seq_lock:
        _seq = 0

def fix_now():
    return datetime.now(timezone.utc).strftime("%Y%m%d-%H:%M:%S")

def fld(tag, val):
    return f"{tag}={val}{SOH}"

def checksum(raw):
    return str(sum(ord(c) for c in raw) % 256).zfill(3)

def _wrap(body):
    begin = fld(8, "FIX.4.2")
    blen  = fld(9, len(begin) + len(body))
    raw   = begin + blen + body
    raw  += fld(10, checksum(raw))
    return raw.encode("ascii")

def parse_fix(raw: bytes) -> dict:
    fields = {}
    for part in raw.decode("ascii", errors="replace").split(SOH):
        if "=" in part:
            k, _, v = part.partition("=")
            try: fields[int(k)] = v
            except ValueError: pass
    return fields

def pretty(raw: bytes) -> str:
    return raw.decode("ascii", errors="replace").replace(SOH, " │ ")

def build_nos(sender, target, symbol, side, qty, price):
    now = fix_now(); oid = f"ORD-{int(time.time()*1000)}"
    return _wrap(
        fld(35,"D")+fld(49,sender)+fld(56,target)+fld(34,next_seq())+fld(52,now)
        +fld(11,oid)+fld(21,1)+fld(55,symbol)+fld(54,side)+fld(60,now)
        +fld(38,int(qty))+fld(40,2)+fld(44,f"{price:.4f}")
    )

def build_md_request(sender, target, symbol, depth=1):
    rid = f"MDR-{int(time.time()*1000)}"; now = fix_now()
    return _wrap(
        fld(35,"V")+fld(49,sender)+fld(56,target)+fld(34,next_seq())+fld(52,now)
        +fld(262,rid)+fld(263,1)+fld(264,depth)
        +fld(267,2)+fld(269,0)+fld(269,1)+fld(146,1)+fld(55,symbol)
    )

def build_md_snapshot(sender, target, symbol, bid, ask, qty):
    rid = f"MDR-{int(time.time()*1000)}"; now = fix_now()
    return _wrap(
        fld(35,"W")+fld(49,sender)+fld(56,target)+fld(34,next_seq())+fld(52,now)
        +fld(262,rid)+fld(55,symbol)+fld(268,2)
        +fld(269,0)+fld(270,f"{bid:.4f}")+fld(271,int(qty))
        +fld(269,1)+fld(270,f"{ask:.4f}")+fld(271,int(qty))
    )

def build_md_incremental(sender, target, symbol, action, entry_type, price, qty):
    now = fix_now()
    return _wrap(
        fld(35,"X")+fld(49,sender)+fld(56,target)+fld(34,next_seq())+fld(52,now)
        +fld(268,1)+fld(279,action)+fld(269,entry_type)
        +fld(55,symbol)+fld(270,f"{price:.4f}")+fld(271,int(qty))
    )

def build_md_reject(sender, target, req_id, reason_code=0, reason_text=""):
    now = fix_now()
    body = (fld(35,"Y")+fld(49,sender)+fld(56,target)+fld(34,next_seq())+fld(52,now)
            +fld(262,req_id)+fld(281,reason_code))
    if reason_text: body += fld(58, reason_text)
    return _wrap(body)


# ─────────────────────────────────────────────────────────────────────────────
#  Order book model
# ─────────────────────────────────────────────────────────────────────────────
class OrderBook:
    MAX_TRADES = 20
    MAX_LEVELS = 8

    def __init__(self):
        self.bids           = {}
        self.asks           = {}
        self.trades         = []
        self.symbol         = "—"
        self.last           = None
        self.pending_orders = {}
        self._lock          = threading.Lock()

    def register_order(self, clord_id, price, qty, side):
        with self._lock:
            self.pending_orders[clord_id] = {"price": price, "qty": qty, "side": side}

    def _sf(self, v):
        try: return float(v)
        except: return 0.0

    def apply(self, fields: dict):
        msg_type = fields.get(35, "")
        with self._lock:
            sym = fields.get(55)
            if sym: self.symbol = sym

            if msg_type == "8":
                ord_status = fields.get(39, "")
                side       = fields.get(54, "")
                clord_id   = fields.get(11, "")
                last_px    = self._sf(fields.get(31))
                last_qty   = self._sf(fields.get(32))
                qty        = self._sf(fields.get(38))
                leaves_qty = self._sf(fields.get(151))
                price      = self._sf(fields.get(44) or fields.get(6))
                cached     = self.pending_orders.get(clord_id)
                if price == 0 and cached: price = cached["price"]
                if side  == "" and cached: side  = cached["side"]

                def _add(px, q):
                    if side == "1": self.bids[px] = self.bids.get(px, 0) + q
                    else:           self.asks[px] = self.asks.get(px, 0) + q

                def _remove(px, q):
                    if side == "1": self._rm(self.bids, px, q)
                    else:           self._rm(self.asks, px, q)

                def _trade(px, q):
                    if px > 0 and q > 0:
                        self.last = px
                        ts = datetime.now().strftime("%H:%M:%S")
                        self.trades.insert(0, (px, q, "B" if side == "1" else "S", ts))
                        self.trades = self.trades[:self.MAX_TRADES]

                if ord_status in ("0", "A"):
                    # New: order rests in the book at limit price
                    if price > 0 and qty > 0:
                        _add(price, qty)
                        print(f"[BOOK] 39=0 side={side} price={price} qty={qty} → bids={dict(self.bids)} asks={dict(self.asks)}")

                elif ord_status == "1":
                    # Partially filled: record trade, reduce resting qty
                    _trade(last_px, last_qty)
                    if price > 0:
                        if leaves_qty > 0:
                            # Set level to exact remaining open qty
                            if side == "1": self.bids[price] = leaves_qty
                            else:           self.asks[price] = leaves_qty
                        else:
                            _remove(price, last_qty)

                elif ord_status == "2":
                    # Filled: record trade, remove resting level entirely
                    fill_px  = last_px if last_px > 0 else price
                    fill_qty = last_qty if last_qty > 0 else qty
                    _trade(fill_px, fill_qty)
                    # Remove the qty that was filled from the aggressor's side.
                    # Also remove from the opposite (passive) side in case the
                    # server only sends one exec report for the match (no 39=0
                    # was sent for the passive resting order).
                    if fill_px > 0:
                        self._rm(self.bids, fill_px, fill_qty)
                        self._rm(self.asks, fill_px, fill_qty)
                    print(f"[BOOK] 39=2 side={side} fill_px={fill_px} fill_qty={fill_qty} → bids={dict(self.bids)} asks={dict(self.asks)}")
                    self.pending_orders.pop(clord_id, None)

                elif ord_status in ("3", "4", "C"):
                    # Cancelled/expired: remove from book
                    if price > 0 and qty > 0: _remove(price, qty)
                    self.pending_orders.pop(clord_id, None)

            elif msg_type == "X":
                action     = int(fields.get(279, 0))
                entry_type = fields.get(269, "0")
                px  = self._sf(fields.get(270))
                qty = self._sf(fields.get(271))
                book = self.bids if entry_type == "0" else self.asks
                if action == 0:   book[px] = book.get(px, 0) + qty
                elif action == 1:
                    if qty > 0: book[px] = qty
                    else: book.pop(px, None)
                elif action == 2: book.pop(px, None)

    def apply_md_snapshot(self, raw: bytes):
        parts = raw.decode("ascii", errors="replace").split(SOH)
        with self._lock:
            self.bids.clear(); self.asks.clear()
            cur_type = None; cur_px = None
            for part in parts:
                if "=" not in part: continue
                k, _, v = part.partition("=")
                try: tag = int(k)
                except: continue
                if tag == 55:    self.symbol = v
                elif tag == 269: cur_type = v; cur_px = None
                elif tag == 270: cur_px = float(v) if v else None
                elif tag == 271 and cur_px is not None:
                    q = float(v) if v else 0
                    if cur_type == "0":   self.bids[cur_px] = q
                    elif cur_type == "1": self.asks[cur_px] = q

    def _rm(self, book, price, qty):
        if price in book:
            book[price] = max(0, book[price] - qty)
            if book[price] == 0: del book[price]

    def snapshot(self):
        with self._lock:
            return {
                "symbol": self.symbol,
                "last":   self.last,
                "bids":   sorted(self.bids.items(), reverse=True)[:self.MAX_LEVELS],
                "asks":   sorted(self.asks.items())[:self.MAX_LEVELS],
                "trades": list(self.trades[:self.MAX_TRADES]),
            }

    def clear(self):
        with self._lock:
            self.bids.clear(); self.asks.clear()
            self.trades.clear(); self.pending_orders.clear()
            self.last = None; self.symbol = "—"


# ─────────────────────────────────────────────────────────────────────────────
#  FIX connection manager
# ─────────────────────────────────────────────────────────────────────────────
class FIXConnection:
    def __init__(self, book: OrderBook, event_bus):
        self.book      = book
        self._bus      = event_bus   # callable(event_type, data)
        self._sock     = None
        self._stop     = threading.Event()
        self._lock     = threading.Lock()
        self.connected = False
        self.config    = {"host": "127.0.0.1", "port": 9876,
                          "sender": "CLIENT1",  "target": "SERVER1"}

    def connect(self, host, port, sender, target):
        if self.connected:
            self._bus("log", {"label": "ERROR", "body": "Already connected.", "tag": "err"})
            return
        self.config = {"host": host, "port": port, "sender": sender, "target": target}
        threading.Thread(target=self._connect_worker, daemon=True).start()

    def _connect_worker(self):
        host, port = self.config["host"], self.config["port"]
        try:
            sock = socket.create_connection((host, port), timeout=8)
            sock.settimeout(None)
            with self._lock:
                self._sock = sock
                self._stop.clear()
                self.connected = True
            self._bus("status", {"connected": True, "host": host, "port": port})
            self._bus("log", {"label": "INFO",
                              "body": f"Connected to {host}:{port}. Listening …", "tag": "info"})
            threading.Thread(target=self._reader_loop, daemon=True).start()
        except (OSError, ConnectionRefusedError) as exc:
            self._bus("log", {"label": "ERROR",
                              "body": f"Cannot connect to {host}:{port} — {exc}", "tag": "err"})

    def disconnect(self):
        self._stop.set()
        with self._lock:
            sock, self._sock = self._sock, None
            self.connected = False
        if sock:
            try: sock.shutdown(socket.SHUT_RDWR); sock.close()
            except OSError: pass
        self._bus("status", {"connected": False})
        self._bus("log", {"label": "INFO", "body": "Disconnected.", "tag": "info"})

    def send(self, msg: bytes, label: str):
        with self._lock:
            sock = self._sock
        if not sock:
            self._bus("log", {"label": "ERROR", "body": "Not connected.", "tag": "err"})
            return False
        try:
            sock.sendall(msg)
            self._bus("log", {"label": label, "body": pretty(msg), "tag": "send"})
            return True
        except OSError as exc:
            self._bus("log", {"label": "ERROR", "body": f"Send failed: {exc}", "tag": "err"})
            self.disconnect()
            return False

    def _reader_loop(self):
        buf = b""
        while not self._stop.is_set():
            try:
                with self._lock:
                    sock = self._sock
                if not sock: break
                sock.settimeout(0.05)
                try:
                    chunk = sock.recv(4096)
                    if not chunk:
                        self._bus("log", {"label": "INFO",
                                          "body": "Server closed connection.", "tag": "info"})
                        self.connected = False
                        self._bus("status", {"connected": False})
                        return
                    buf += chunk
                except socket.timeout:
                    pass

                emitted = False
                while True:
                    idx = buf.find(b"\x0110=")
                    if idx == -1: break
                    end = buf.find(b"\x01", idx + 1)
                    if end == -1: break
                    msg, buf = buf[:end+1], buf[end+1:]
                    self._handle(msg); emitted = True

                if not emitted and buf:
                    nxt = buf.find(b"8=FIX", 1)
                    if nxt != -1:
                        msg, buf = buf[:nxt], buf[nxt:]
                        self._handle(msg)
                    elif buf and sock and sock.fileno() != -1:
                        try:
                            sock.settimeout(0.1)
                            extra = sock.recv(4096)
                            if extra: buf += extra
                            else: self._handle(buf); buf = b""
                        except socket.timeout:
                            self._handle(buf); buf = b""
                        finally:
                            if sock: sock.settimeout(None)

            except OSError:
                if not self._stop.is_set():
                    self.connected = False
                    self._bus("status", {"connected": False})
                    self._bus("log", {"label": "INFO",
                                      "body": "Connection lost.", "tag": "info"})
                return

    def _handle(self, raw: bytes):
        fields   = parse_fix(raw)
        msg_type = fields.get(35, "?")
        if msg_type == "W":
            self.book.apply_md_snapshot(raw)
        else:
            self.book.apply(fields)
        labels = {
            "8": "◀ EXEC REPORT (8)", "W": "◀ MD SNAPSHOT (W)",
            "X": "◀ MD INCREMENTAL (X)", "Y": "◀ MD REJECT (Y)",
            "V": "◀ MD REQUEST (V)",  "0": "◀ HEARTBEAT (0)",
            "A": "◀ LOGON (A)",       "5": "◀ LOGOUT (5)",
        }
        label = labels.get(msg_type, f"◀ MSG ({msg_type})")
        self._bus("log",  {"label": label, "body": pretty(raw), "tag": "feed"})
        self._bus("book", self.book.snapshot())


# ─────────────────────────────────────────────────────────────────────────────
#  HTML / CSS / JS  — the entire frontend as a string
# ─────────────────────────────────────────────────────────────────────────────
HTML = r"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<title>FIX 4.2 Terminal</title>
<style>
  :root {
    --bg:#0d0f14; --panel:#13161e; --panel2:#0f111a; --border:#1e2330;
    --green:#00e676; --red:#ff3d57; --blue:#2979ff; --gold:#ffd740;
    --teal:#00bcd4; --text:#e8ecf4; --muted:#5a6070;
    --bid-bg:#0a1a10; --ask-bg:#1a0a0e; --bid-bar:#003a18; --ask-bar:#3a0a10;
  }
  *{box-sizing:border-box;margin:0;padding:0}
  body{background:var(--bg);color:var(--text);font-family:'Courier New',monospace;
       font-size:13px;height:100vh;display:flex;flex-direction:column;overflow:hidden}
  #topbar{background:var(--panel);height:42px;display:flex;align-items:center;
          padding:0 18px;justify-content:space-between;flex-shrink:0;
          border-bottom:1px solid var(--border)}
  #topbar h1{color:var(--green);font-size:13px;letter-spacing:.05em}
  #status-dot{font-size:13px;color:var(--red)}
  #status-dot.on{color:var(--green)}
  #body{display:flex;flex:1;overflow:hidden}
  .col{display:flex;flex-direction:column;overflow:hidden}
  #col-left{width:300px;min-width:260px;background:var(--panel);
             border-right:1px solid var(--border);flex-shrink:0;overflow-y:auto}
  #col-book{width:300px;min-width:260px;background:var(--panel2);
             border-right:1px solid var(--border);flex-shrink:0;overflow:hidden;
             display:flex;flex-direction:column}
  #col-log{flex:1;background:var(--bg);display:flex;flex-direction:column;min-width:0}
  .stitle{font-size:9px;letter-spacing:.12em;color:var(--muted);
          padding:14px 16px 4px;text-transform:uppercase}
  .ssep{height:1px;background:var(--border);margin:0 16px 8px}
  .frow{display:flex;align-items:center;padding:3px 16px}
  .frow label{color:var(--muted);width:110px;flex-shrink:0;font-size:11px}
  .frow input{background:#1a1e2a;border:1px solid var(--border);color:var(--text);
              font-family:'Courier New',monospace;font-size:11px;
              padding:5px 6px;flex:1;outline:none}
  .frow input:focus{border-color:var(--blue)}
  .frow input:disabled{opacity:.4;cursor:not-allowed}
  button{font-family:'Courier New',monospace;cursor:pointer;border:none;outline:none}
  button:disabled{opacity:.4;cursor:not-allowed}
  .btn{display:block;width:calc(100% - 32px);margin:4px 16px;
       padding:8px 0;font-size:12px;font-weight:bold;letter-spacing:.04em}
  .btn-connect{background:var(--blue);color:var(--text)}
  .btn-send{background:var(--green);color:#0d0f14;font-size:14px;
            padding:10px 0;margin-top:12px}
  .btn-send.sell{background:var(--red)}
  .side-row{display:flex;align-items:center;padding:6px 16px}
  .side-row label{color:var(--muted);width:110px;flex-shrink:0;font-size:11px}
  .sbtn{padding:5px 0;width:80px;font-size:11px;font-weight:bold}
  #btn-buy{background:var(--green);color:#0d0f14;margin-right:6px}
  #btn-sell{background:#1a1e2a;color:var(--muted)}
  .md-btn{display:block;width:calc(100% - 32px);margin:2px 16px;
          padding:6px 10px;text-align:left;background:#0e1520;
          color:var(--teal);font-size:11px;font-weight:bold}
  .footer-row{display:flex;justify-content:space-between;padding:10px 16px 12px}
  .ghost{background:transparent;color:var(--muted);font-size:10px;
         font-family:'Courier New',monospace;padding:3px 6px}
  .ghost:hover{color:var(--text)}
  /* book */
  #book-hdr{display:flex;justify-content:space-between;align-items:center;padding:10px 10px 4px}
  #book-sym{color:var(--teal);font-size:11px;font-weight:bold}
  #book-last{color:var(--gold);font-size:10px}
  .bk-level{display:flex;align-items:center;height:22px;margin:1px 8px;
             position:relative;overflow:hidden}
  .bk-level .bar{position:absolute;top:0;bottom:0;left:0}
  .bid-lv{background:var(--bid-bg)}.bid-lv .bar{background:var(--bid-bar)}
  .ask-lv{background:var(--ask-bg)}.ask-lv .bar{background:var(--ask-bar)}
  .lv-qty{flex:1;text-align:right;font-size:11px;padding-right:6px;position:relative;z-index:1}
  .lv-px{width:90px;text-align:right;font-size:11px;font-weight:bold;
          padding-right:4px;position:relative;z-index:1}
  .bid-lv .lv-px,.bid-lv .lv-qty{color:var(--green)}
  .ask-lv .lv-px,.ask-lv .lv-qty{color:var(--red)}
  #spread-row{height:26px;display:flex;align-items:center;justify-content:center;
              font-size:10px;color:var(--muted);
              border-top:1px solid var(--border);border-bottom:1px solid var(--border);
              margin:2px 0;flex-shrink:0}
  #trades-hdr{font-size:9px;color:var(--muted);letter-spacing:.1em;
              padding:6px 10px 2px;flex-shrink:0}
  #trades-list{flex:1;overflow-y:auto;padding:0 8px 8px}
  .tr-row{display:flex;gap:6px;font-size:11px;line-height:1.8}
  .tr-row .td{width:14px;font-weight:bold}
  .tr-row .tp{width:90px;text-align:right;font-weight:bold}
  .tr-row .tq{width:70px;text-align:right;color:var(--muted)}
  .tr-row .tt{color:var(--muted);font-size:10px}
  .tr-buy .td,.tr-buy .tp{color:var(--green)}
  .tr-sell .td,.tr-sell .tp{color:var(--red)}
  /* log */
  #log-hdr{display:flex;justify-content:space-between;align-items:center;
           padding:10px 12px 4px;flex-shrink:0}
  #log-hdr span{font-size:9px;color:var(--muted);letter-spacing:.1em}
  #log-box{flex:1;overflow-y:auto;padding:0 8px 8px;font-size:10px;background:#0a0c10}
  .le{margin-bottom:10px}
  .lts{color:var(--muted)}
  .llbl{display:block;margin-bottom:2px}
  .lbody{color:#9e9e9e;word-break:break-all;line-height:1.5}
  .tsend{color:var(--green)}.tfeed{color:var(--gold)}.tmd{color:var(--teal)}
  .terr{color:var(--red)}.tinfo{color:var(--blue)}
  ::-webkit-scrollbar{width:4px}
  ::-webkit-scrollbar-track{background:transparent}
  ::-webkit-scrollbar-thumb{background:var(--border);border-radius:2px}
  /* chart */
  #col-chart{width:280px;min-width:220px;background:var(--panel2);
             border-left:1px solid var(--border);flex-shrink:0;
             display:flex;flex-direction:column}
  #chart-hdr{display:flex;justify-content:space-between;align-items:center;
             padding:10px 12px 4px;flex-shrink:0}
  #chart-hdr span{font-size:9px;color:var(--muted);letter-spacing:.1em;text-transform:uppercase}
  #chart-wrap{flex:1;position:relative;overflow:hidden;padding:4px 8px 8px}
  #price-canvas{display:block;width:100%;height:100%}
  /* dialog */
  #dlg-ov{display:none;position:fixed;inset:0;background:rgba(0,0,0,.6);
           align-items:center;justify-content:center;z-index:100}
  #dlg-box{background:var(--panel);padding:24px 28px;min-width:360px;
           border:1px solid var(--border)}
  #dlg-title{color:var(--teal);font-size:12px;font-weight:bold;
             margin-bottom:16px;letter-spacing:.05em}
  .dlg-row{display:flex;align-items:center;margin-bottom:8px}
  .dlg-row label{color:var(--muted);width:130px;font-size:11px}
  .dlg-row input,.dlg-row select{
    background:#1a1e2a;border:1px solid var(--border);color:var(--text);
    font-family:'Courier New',monospace;font-size:11px;
    padding:5px 6px;flex:1;outline:none}
  .dlg-foot{display:flex;justify-content:flex-end;gap:8px;margin-top:18px}
  .dlg-foot button{padding:7px 18px;font-family:'Courier New',monospace;
                   cursor:pointer;border:none;font-size:11px}
</style>
</head>
<body>
<div id="topbar">
  <h1>&#x2B21; &nbsp;FIX 4.2 &nbsp;ORDER &amp; MARKET DATA TERMINAL</h1>
  <span id="status-dot">&#9679; DISCONNECTED</span>
</div>
<div id="body">

<!-- LEFT -->
<div id="col-left" class="col">
  <div class="stitle">CONNECTION</div><div class="ssep"></div>
  <div class="frow"><label>Host</label>        <input id="c-host"   value="127.0.0.1"></div>
  <div class="frow"><label>Port</label>        <input id="c-port"   value="9876"></div>
  <div class="frow"><label>SenderCompID</label><input id="c-sender" value="CLIENT1"></div>
  <div class="frow"><label>TargetCompID</label><input id="c-target" value="SERVER1"></div>
  <button id="btn-connect" class="btn btn-connect" onclick="toggleConnect()">&#x23FB; &nbsp;CONNECT</button>

  <div class="stitle" style="margin-top:6px">ORDER ENTRY</div><div class="ssep"></div>
  <div class="frow"><label>Symbol</label>     <input id="o-sym"   value="AAPL"></div>
  <div class="frow"><label>Quantity</label>   <input id="o-qty"   value="100"></div>
  <div class="frow"><label>Limit Price</label><input id="o-price" value="150.00"></div>
  <div class="side-row">
    <label>Side</label>
    <button id="btn-buy"  class="sbtn" onclick="setSide('BUY')">BUY</button>
    <button id="btn-sell" class="sbtn" onclick="setSide('SELL')">SELL</button>
  </div>
  <button id="btn-send" class="btn btn-send" onclick="sendOrder()" disabled>&#9654; &nbsp;BUY ORDER</button>

  <div class="stitle" style="margin-top:10px">MARKET DATA</div><div class="ssep"></div>
  <button class="md-btn" id="md-V" onclick="openDlg('V')" disabled>V &nbsp; Market Data Request</button>
  <button class="md-btn" id="md-W" onclick="openDlg('W')" disabled>W &nbsp; MD Snapshot Full Refresh</button>
  <button class="md-btn" id="md-X" onclick="openDlg('X')" disabled>X &nbsp; MD Incremental Refresh</button>
  <button class="md-btn" id="md-Y" onclick="openDlg('Y')" disabled>Y &nbsp; MD Request Reject</button>

  <div class="footer-row">
    <button class="ghost" onclick="api({action:'clear_book'})">clear book</button>
    <button class="ghost" onclick="api({action:'reset_seq'})">reset seq</button>
  </div>
</div>

<!-- BOOK -->
<div id="col-book">
  <div id="book-hdr">
    <span id="book-sym">ORDER BOOK &nbsp;&#8212;</span>
    <span id="book-last">last: &#8212;</span>
  </div>
  <div style="height:1px;background:var(--border);margin:0 8px 4px"></div>
  <div id="asks"></div>
  <div id="spread-row">spread: &#8212;</div>
  <div id="bids"></div>
  <div id="trades-hdr">LAST TRADES</div>
  <div id="trades-list"></div>
</div>

<!-- LOG -->
<div id="col-log" class="col">
  <div id="log-hdr">
    <span>MESSAGE LOG</span>
    <button class="ghost" onclick="document.getElementById('log-box').innerHTML=''">CLEAR</button>
  </div>
  <div id="log-box"></div>
</div>

<!-- CHART -->
<div id="col-chart">
  <div id="chart-hdr">
    <span>PRICE CHART</span>
    <button class="ghost" onclick="clearChart()">CLEAR</button>
  </div>
  <div id="chart-wrap">
    <canvas id="price-canvas"></canvas>
  </div>
</div>
</div>

<!-- DIALOG -->
<div id="dlg-ov">
  <div id="dlg-box">
    <div id="dlg-title"></div>
    <div id="dlg-fields"></div>
    <div class="dlg-foot">
      <button style="background:var(--border);color:var(--muted)" onclick="closeDlg()">CANCEL</button>
      <button style="background:var(--blue);color:var(--text);font-weight:bold" onclick="submitDlg()">SEND</button>
    </div>
  </div>
</div>

<script>
'use strict';
let side = 'BUY', dlgType = null;
const LEVELS = 8;
let logCount = 0;

// ── helpers ──────────────────────────────────────────────────────────────────
const ge = id => document.getElementById(id);
const fmt = v => parseFloat(v).toFixed(4);
const esc = s => String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');

async function api(body) {
  try {
    const r = await fetch('/api/action', {
      method:'POST', headers:{'Content-Type':'application/json'},
      body: JSON.stringify(body)
    });
    if (!r.ok) console.error('API error', r.status);
  } catch(e) { console.error('fetch failed', e); }
}

// ── polling — replaces SSE, no CORS, works on all browsers ───────────────────
let lastLogIdx = 0;

async function poll() {
  try {
    const r = await fetch('/api/state?since=' + lastLogIdx);
    if (r.ok) {
      const d = await r.json();
      handleStatus(d.status);
      if (d.book) {
        console.log('book bids:', JSON.stringify(d.book.bids), 'asks:', JSON.stringify(d.book.asks));
        renderBook(d.book);
      }
      if (d.logs && d.logs.length) {
        d.logs.forEach(appendLog);
        lastLogIdx = d.log_idx;
      }
    }
  } catch(e) { console.error('poll error', e); }
  setTimeout(poll, 250);
}
poll();

// ── connection ────────────────────────────────────────────────────────────────
function toggleConnect() {
  const btn = ge('btn-connect');
  const connected = btn.dataset.connected === '1';
  if (!connected) {
    api({ action:'connect',
          host:   ge('c-host').value.trim(),
          port:   parseInt(ge('c-port').value.trim()),
          sender: ge('c-sender').value.trim(),
          target: ge('c-target').value.trim() });
  } else {
    api({action:'disconnect'});
  }
}

function handleStatus(connected) {
  const dot = ge('status-dot');
  const btn = ge('btn-connect');
  btn.dataset.connected = connected ? '1' : '0';
  if (connected) {
    dot.textContent = '● CONNECTED'; dot.className = 'on';
    btn.textContent = '⏼  DISCONNECT';
    btn.style.background = '#3a1010'; btn.style.color = 'var(--red)';
    ['c-host','c-port','c-sender','c-target'].forEach(id => ge(id).disabled = true);
    ge('btn-send').disabled = false;
    document.querySelectorAll('.md-btn').forEach(b => b.disabled = false);
  } else {
    dot.textContent = '● DISCONNECTED'; dot.className = '';
    btn.textContent = '⏻  CONNECT';
    btn.style.background = 'var(--blue)'; btn.style.color = 'var(--text)';
    ['c-host','c-port','c-sender','c-target'].forEach(id => ge(id).disabled = false);
    ge('btn-send').disabled = true;
    document.querySelectorAll('.md-btn').forEach(b => b.disabled = true);
  }
}

// ── side toggle ───────────────────────────────────────────────────────────────
function setSide(s) {
  side = s;
  if (s === 'BUY') {
    ge('btn-buy').style.cssText  = 'background:var(--green);color:#0d0f14';
    ge('btn-sell').style.cssText = 'background:#1a1e2a;color:var(--muted)';
    ge('btn-send').textContent = '▶  BUY ORDER';
    ge('btn-send').className = 'btn btn-send';
  } else {
    ge('btn-sell').style.cssText = 'background:var(--red);color:#0d0f14';
    ge('btn-buy').style.cssText  = 'background:#1a1e2a;color:var(--muted)';
    ge('btn-send').textContent = '▶  SELL ORDER';
    ge('btn-send').className = 'btn btn-send sell';
  }
}

// ── send order ────────────────────────────────────────────────────────────────
async function sendOrder() {
  const btn = ge('btn-send');
  btn.disabled = true; btn.textContent = 'SENDING…';
  await api({ action:'order',
    symbol: ge('o-sym').value.trim().toUpperCase(),
    qty:    parseFloat(ge('o-qty').value),
    price:  parseFloat(ge('o-price').value),
    side:   side === 'BUY' ? '1' : '2' });
  btn.disabled = false;
  btn.textContent = side === 'BUY' ? '▶  BUY ORDER' : '▶  SELL ORDER';
}

// ── MD dialogs ────────────────────────────────────────────────────────────────
const MD_DEFS = {
  V:{ title:'MARKET DATA REQUEST  (V)',
      fields:[['Symbol','symbol','AAPL'],['Market Depth','depth','1']] },
  W:{ title:'MD SNAPSHOT FULL REFRESH  (W)',
      fields:[['Symbol','symbol','AAPL'],['Bid','bid','149.50'],['Ask','ask','150.50'],['Qty','qty','100']] },
  X:{ title:'MD INCREMENTAL REFRESH  (X)',
      fields:[['Symbol','symbol','AAPL'],['Price','price','150.00'],['Qty','qty','100']],
      selects:{ action:['0 = New','1 = Change','2 = Delete'],
                entry_type:['0 = Bid','1 = Offer','2 = Trade'] } },
  Y:{ title:'MD REQUEST REJECT  (Y)',
      fields:[['MDReqID','req_id','MDR-0'],['Text (opt)','text','']],
      selects:{ reason_code:['0 = Unknown Symbol','1 = Dup MDReqID','2 = Insuf. Bandwidth',
        '3 = Insuf. Perms','4 = Unsup. SubReqType','5 = Unsup. MarketDepth',
        '6 = Unsup. MDUpdateType','7 = Unsup. AggBook','8 = Unsup. MDEntryType'] } },
};

function openDlg(type) {
  dlgType = type;
  const def = MD_DEFS[type];
  ge('dlg-title').textContent = def.title;
  const w = ge('dlg-fields'); w.innerHTML = '';
  def.fields.forEach(([lbl,id,val]) => {
    w.innerHTML += `<div class="dlg-row"><label>${esc(lbl)}</label>
      <input id="di-${id}" value="${esc(val)}"></div>`;
  });
  (def.selects ? Object.entries(def.selects) : []).forEach(([id,opts]) => {
    w.innerHTML += `<div class="dlg-row"><label>${id.replace(/_/g,' ')}</label>
      <select id="di-${id}">${opts.map(o=>`<option>${esc(o)}</option>`).join('')}</select></div>`;
  });
  ge('dlg-ov').style.display = 'flex';
}
function closeDlg() { ge('dlg-ov').style.display = 'none'; }
async function submitDlg() {
  const def = MD_DEFS[dlgType];
  const p = {action:'md', type:dlgType};
  def.fields.forEach(([,id]) => { p[id] = ge('di-'+id).value.trim(); });
  (def.selects ? Object.keys(def.selects) : []).forEach(id => {
    p[id] = ge('di-'+id).value.split('=')[0].trim();
  });
  closeDlg();
  await api(p);
}

// ── log ───────────────────────────────────────────────────────────────────────
function appendLog(entry) {
  const box = ge('log-box');
  const el  = document.createElement('div');
  el.className = 'le';
  el.innerHTML = `<span class="lts">[${esc(entry.ts)}]</span>
    <span class="llbl t${esc(entry.tag)}">${esc(entry.label)}</span>
    <div class="lbody">${esc(entry.body)}</div>`;
  box.appendChild(el);
  // keep at most 200 entries in DOM
  while (box.children.length > 200) box.removeChild(box.firstChild);
  box.scrollTop = box.scrollHeight;
}

// ── order book ────────────────────────────────────────────────────────────────
function renderBook(d) {
  ge('book-sym').textContent  = 'ORDER BOOK  ' + d.symbol;
  ge('book-last').textContent = d.last != null ? 'last: ' + fmt(d.last) : 'last: —';
  const allQ = [...d.bids,...d.asks].map(r=>r[1]);
  const maxQ = allQ.length ? Math.max(...allQ) : 1;
  renderLevels('asks', [...d.asks.slice(0,LEVELS)].reverse(), 'ask', maxQ);
  if (d.bids.length && d.asks.length) {
    const sp = d.asks[0][0] - d.bids[0][0], mid = (d.asks[0][0]+d.bids[0][0])/2;
    ge('spread-row').textContent = 'spread: '+fmt(sp)+'   mid: '+fmt(mid);
  } else { ge('spread-row').textContent = 'spread: —'; }
  renderLevels('bids', d.bids.slice(0,LEVELS), 'bid', maxQ);
  ge('trades-list').innerHTML = d.trades.map(([px,qty,s,ts])=>`
    <div class="tr-row tr-${s==='B'?'buy':'sell'}">
      <span class="td">${s==='B'?'▲':'▼'}</span>
      <span class="tp">${fmt(px)}</span>
      <span class="tq">${Math.round(qty).toLocaleString()}</span>
      <span class="tt">${esc(ts)}</span>
    </div>`).join('');
  // update chart with latest trades
  if (d.trades && d.trades.length) updateChart(d.trades);
}
function renderLevels(id, levels, cls, maxQ) {
  while(levels.length < LEVELS) levels.push(null);
  ge(id).innerHTML = levels.map(row => {
    if(!row) return `<div class="bk-level ${cls}-lv"></div>`;
    const [px,qty] = row, pct = (qty/maxQ*100).toFixed(1);
    return `<div class="bk-level ${cls}-lv">
      <div class="bar" style="width:${pct}%"></div>
      <span class="lv-qty">${Math.round(qty).toLocaleString()}</span>
      <span class="lv-px">${fmt(px)}</span></div>`;
  }).join('');
}

// ── price chart ───────────────────────────────────────────────────────────────
const MAX_CHART_PTS = 200;
let chartPts = [];   // [{price, qty, side, ts}]
let lastTradeCount = 0;

function clearChart() {
  chartPts = [];
  lastTradeCount = 0;
  drawChart();
}

function updateChart(trades) {
  // trades from server are newest-first; we want oldest-first for the chart
  // only append genuinely new points (compare by count)
  if (trades.length === lastTradeCount) return;
  // New trades appeared — diff from the end
  const newCount = trades.length - lastTradeCount;
  if (newCount > 0 && lastTradeCount > 0) {
    // server list is newest-first, so new arrivals are at index 0..newCount-1
    const incoming = trades.slice(0, newCount).reverse();
    incoming.forEach(([px, qty, side, ts]) => {
      chartPts.push({price: parseFloat(px), qty, side, ts});
    });
  } else if (lastTradeCount === 0) {
    // first load — add all, oldest first
    [...trades].reverse().forEach(([px, qty, side, ts]) => {
      chartPts.push({price: parseFloat(px), qty, side, ts});
    });
  }
  lastTradeCount = trades.length;
  if (chartPts.length > MAX_CHART_PTS) chartPts = chartPts.slice(-MAX_CHART_PTS);
  drawChart();
}

function drawChart() {
  const wrap   = ge('chart-wrap');
  const canvas = ge('price-canvas');
  const dpr    = window.devicePixelRatio || 1;
  const W      = wrap.clientWidth  || 260;
  const H      = wrap.clientHeight || 300;
  canvas.width  = W * dpr;
  canvas.height = H * dpr;
  canvas.style.width  = W + 'px';
  canvas.style.height = H + 'px';
  const ctx = canvas.getContext('2d');
  ctx.scale(dpr, dpr);

  // background
  ctx.fillStyle = '#0a0c10';
  ctx.fillRect(0, 0, W, H);

  if (chartPts.length < 2) {
    ctx.fillStyle = '#5a6070';
    ctx.font = '10px Courier New';
    ctx.textAlign = 'center';
    ctx.fillText('waiting for trades…', W/2, H/2);
    return;
  }

  const PAD_L = 52, PAD_R = 8, PAD_T = 14, PAD_B = 28;
  const cW = W - PAD_L - PAD_R;
  const cH = H - PAD_T - PAD_B;

  const prices = chartPts.map(p => p.price);
  let minP = Math.min(...prices), maxP = Math.max(...prices);
  // ensure some vertical range so flat lines don't sit on the axis
  if (maxP === minP) { minP -= 0.5; maxP += 0.5; }
  const range = maxP - minP;
  // add 10% padding top/bottom
  const lo = minP - range * 0.1, hi = maxP + range * 0.1;

  const xOf = i => PAD_L + (i / (chartPts.length - 1)) * cW;
  const yOf = p => PAD_T + (1 - (p - lo) / (hi - lo)) * cH;

  // ── grid lines ──
  ctx.strokeStyle = '#1e2330';
  ctx.lineWidth   = 1;
  const nGrid = 4;
  for (let i = 0; i <= nGrid; i++) {
    const p = lo + (hi - lo) * (i / nGrid);
    const y = yOf(p);
    ctx.beginPath(); ctx.moveTo(PAD_L, y); ctx.lineTo(W - PAD_R, y); ctx.stroke();
    // price label
    ctx.fillStyle = '#5a6070';
    ctx.font = '9px Courier New';
    ctx.textAlign = 'right';
    ctx.fillText(p.toFixed(2), PAD_L - 4, y + 3);
  }

  // ── area fill under the line ──
  const grad = ctx.createLinearGradient(0, PAD_T, 0, PAD_T + cH);
  grad.addColorStop(0,   'rgba(0,230,118,0.18)');
  grad.addColorStop(1,   'rgba(0,230,118,0.00)');
  ctx.beginPath();
  ctx.moveTo(xOf(0), yOf(chartPts[0].price));
  chartPts.forEach((pt, i) => { if (i > 0) ctx.lineTo(xOf(i), yOf(pt.price)); });
  ctx.lineTo(xOf(chartPts.length - 1), PAD_T + cH);
  ctx.lineTo(xOf(0), PAD_T + cH);
  ctx.closePath();
  ctx.fillStyle = grad;
  ctx.fill();

  // ── price line ──
  ctx.beginPath();
  ctx.strokeStyle = '#00e676';
  ctx.lineWidth   = 1.5;
  ctx.lineJoin    = 'round';
  chartPts.forEach((pt, i) => {
    if (i === 0) ctx.moveTo(xOf(i), yOf(pt.price));
    else         ctx.lineTo(xOf(i), yOf(pt.price));
  });
  ctx.stroke();

  // ── trade dots ──
  chartPts.forEach((pt, i) => {
    const x = xOf(i), y = yOf(pt.price);
    ctx.beginPath();
    ctx.arc(x, y, 3, 0, Math.PI * 2);
    ctx.fillStyle = pt.side === 'B' ? '#00e676' : '#ff3d57';
    ctx.fill();
  });

  // ── last price line + label ──
  const lastPt = chartPts[chartPts.length - 1];
  const lastY  = yOf(lastPt.price);
  ctx.setLineDash([3, 3]);
  ctx.strokeStyle = '#ffd740';
  ctx.lineWidth   = 1;
  ctx.beginPath(); ctx.moveTo(PAD_L, lastY); ctx.lineTo(W - PAD_R, lastY); ctx.stroke();
  ctx.setLineDash([]);
  // label box
  const lbl = lastPt.price.toFixed(4);
  ctx.font = 'bold 9px Courier New';
  const tw  = ctx.measureText(lbl).width + 8;
  ctx.fillStyle = '#ffd740';
  ctx.fillRect(PAD_L - tw - 2, lastY - 7, tw, 14);
  ctx.fillStyle = '#0d0f14';
  ctx.textAlign = 'right';
  ctx.fillText(lbl, PAD_L - 5, lastY + 4);

  // ── x-axis timestamps (first, mid, last) ──
  ctx.fillStyle = '#5a6070';
  ctx.font = '8px Courier New';
  ctx.textAlign = 'center';
  [[0, PAD_L], [Math.floor((chartPts.length-1)/2), W/2], [chartPts.length-1, W-PAD_R]].forEach(([idx, x]) => {
    if (chartPts[idx]) ctx.fillText(chartPts[idx].ts, x, H - 8);
  });
}

// initial draw (empty state)
window.addEventListener('load', drawChart);
window.addEventListener('resize', drawChart);
</script>
</body></html>
"""



# ─────────────────────────────────────────────────────────────────────────────
#  HTTP request handler
# ─────────────────────────────────────────────────────────────────────────────

# ─────────────────────────────────────────────────────────────────────────────
#  State store — replaces EventBus; browser polls /api/state
# ─────────────────────────────────────────────────────────────────────────────
class StateStore:
    """
    Holds the latest book snapshot + a rolling log buffer.
    The browser polls GET /api/state?since=N and gets back only new log entries.
    No SSE, no keep-alive connections, no browser compatibility issues.
    """
    MAX_LOG = 500

    def __init__(self):
        self._lock  = threading.Lock()
        self._book  = {"symbol":"—","last":None,"bids":[],"asks":[],"trades":[]}
        self._log   = []   # list of {ts, label, body, tag}
        self._idx   = 0    # monotonic counter

    def push_log(self, label: str, body: str, tag: str):
        entry = {
            "ts":    datetime.now().strftime("%H:%M:%S.%f")[:-3],
            "label": label,
            "body":  body,
            "tag":   tag,
        }
        with self._lock:
            self._log.append(entry)
            self._idx += 1
            if len(self._log) > self.MAX_LOG:
                self._log = self._log[-self.MAX_LOG:]

    def push_book(self, snapshot: dict):
        with self._lock:
            self._book = snapshot

    def get_state(self, since: int) -> dict:
        with self._lock:
            total = self._idx
            # Return only entries after 'since'
            start = max(0, len(self._log) - (total - since))
            new_entries = self._log[start:]
            return {
                "status": self._connected,
                "book":   self._book,
                "logs":   new_entries,
                "log_idx": total,
            }

    # connection status tracked separately
    _connected = False
    def set_connected(self, v: bool):
        with self._lock:
            self._connected = v


# ─────────────────────────────────────────────────────────────────────────────
#  HTTP handler — single port, no SSE
# ─────────────────────────────────────────────────────────────────────────────
class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"   # new connection per request — no keep-alive queuing
    store:    StateStore    = None
    fix_conn: FIXConnection = None
    book:     OrderBook     = None

    def log_message(self, *_):
        pass

    # ── auth ─────────────────────────────────────────────────────────────────
    def _check_auth(self) -> bool:
        """Return True if request is authenticated (or no password set)."""
        if not self.server.password_hash:
            return True
        auth = self.headers.get("Authorization", "")
        if not auth.startswith("Basic "):
            return False
        try:
            decoded  = base64.b64decode(auth[6:]).decode()
            _, _, pw = decoded.partition(":")
            pw_hash  = hashlib.sha256(pw.encode()).hexdigest()
            return secrets.compare_digest(pw_hash, self.server.password_hash)
        except Exception:
            return False

    def _require_auth(self):
        """Send 401 and return False if not authenticated."""
        self.send_response(401)
        self.send_header("WWW-Authenticate", 'Basic realm="FIX Terminal"')
        self.send_header("Content-Length", "0")
        self.end_headers()
        return False

    def do_GET(self):
        if not self._check_auth():
            self._require_auth(); return
        path = urlparse(self.path).path

        if path in ("/", "/index.html"):
            body = HTML.encode()
            self.send_response(200)
            self.send_header("Content-Type",   "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        elif path == "/api/state":
            from urllib.parse import parse_qs, urlparse as _up
            qs    = parse_qs(_up(self.path).query)
            since = int(qs.get("since", ["0"])[0])
            data  = self.server.store.get_state(since)
            body  = json.dumps(data).encode()
            self.send_response(200)
            self.send_header("Content-Type",   "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control",  "no-store")
            self.end_headers()
            self.wfile.write(body)

        elif path == "/api/debug":
            # Raw book internals for troubleshooting
            book = self.server.book
            with book._lock:
                data = {
                    "bids":   dict(book.bids),
                    "asks":   dict(book.asks),
                    "pending": {k: v for k, v in book.pending_orders.items()},
                    "trades": list(book.trades[:5]),
                }
            body = json.dumps(data, indent=2).encode()
            self.send_response(200)
            self.send_header("Content-Type",   "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        else:
            self.send_error(404)

    def do_POST(self):
        if not self._check_auth():
            self._require_auth(); return
        if urlparse(self.path).path != "/api/action":
            self.send_error(404); return
        length = int(self.headers.get("Content-Length", 0))
        try:
            data = json.loads(self.rfile.read(length))
        except Exception:
            self.send_error(400); return
        self._dispatch(data)
        body = b'{"ok":true}'
        self.send_response(200)
        self.send_header("Content-Type",   "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _dispatch(self, d):
        conn   = self.server.fix_conn
        store  = self.server.store
        cfg    = conn.config
        sender = d.get("sender") or cfg["sender"]
        target = d.get("target") or cfg["target"]
        action = d.get("action", "")

        if action == "connect":
            conn.connect(d["host"], int(d["port"]), d["sender"], d["target"])

        elif action == "disconnect":
            conn.disconnect()

        elif action == "order":
            symbol = d["symbol"]
            qty    = float(d["qty"])
            price  = float(d["price"])
            side   = d["side"]
            msg    = build_nos(sender, target, symbol, int(side), qty, price)
            clord  = parse_fix(msg).get(11, "")
            self.server.book.register_order(clord, price, qty, side)
            lbl = "BUY" if side == "1" else "SELL"
            conn.send(msg, f"SENT ▶  ({lbl} {int(qty)} {symbol} @ {price:.4f})")

        elif action == "md":
            typ = d.get("type")
            if typ == "V":
                msg = build_md_request(sender, target, d["symbol"], int(d.get("depth",1)))
                conn.send(msg, f"SENT ▶  MD REQUEST (V)  {d['symbol']}")
            elif typ == "W":
                msg = build_md_snapshot(sender, target, d["symbol"],
                                        float(d["bid"]), float(d["ask"]), float(d["qty"]))
                conn.send(msg, f"SENT ▶  MD SNAPSHOT (W)  {d['symbol']}")
            elif typ == "X":
                msg = build_md_incremental(sender, target, d["symbol"],
                                           int(d["action"]), int(d["entry_type"]),
                                           float(d["price"]), float(d["qty"]))
                conn.send(msg, f"SENT ▶  MD INCREMENTAL (X)  {d['symbol']}")
            elif typ == "Y":
                msg = build_md_reject(sender, target, d["req_id"],
                                      int(d.get("reason_code",0)), d.get("text",""))
                conn.send(msg, f"SENT ▶  MD REJECT (Y)  req={d['req_id']}")

        elif action == "reset_seq":
            reset_seq()
            store.push_log("INFO", "Sequence number reset to 0.", "info")

        elif action == "clear_book":
            self.server.book.clear()
            store.push_log("INFO", "Order book cleared.", "info")
            store.push_book(self.server.book.snapshot())


# ─────────────────────────────────────────────────────────────────────────────
#  Entry point
# ─────────────────────────────────────────────────────────────────────────────
def generate_self_signed_cert(cert_file: str, key_file: str):
    """Generate a self-signed certificate using openssl (available on all platforms)."""
    import subprocess
    subprocess.run([
        "openssl", "req", "-x509", "-newkey", "rsa:2048",
        "-keyout", key_file, "-out", cert_file,
        "-days", "365", "-nodes",
        "-subj", "/CN=fix-terminal"
    ], check=True, capture_output=True)
    print(f"  Generated self-signed cert: {cert_file}")


def main():
    ap = argparse.ArgumentParser(description="FIX 4.2 Web Terminal")
    ap.add_argument("--http-port", type=int, default=8080,   help="HTTP/HTTPS port (default: 8080)")
    ap.add_argument("--fix-host",  default="127.0.0.1",      help="FIX server host")
    ap.add_argument("--fix-port",  type=int, default=9876,   help="FIX server port")
    ap.add_argument("--password",  default="",               help="Password for browser login (recommended when exposed to internet)")
    ap.add_argument("--cert",      default="",               help="TLS cert file (.pem). If omitted with --https, auto-generates self-signed.")
    ap.add_argument("--key",       default="",               help="TLS key file (.pem)")
    ap.add_argument("--https",     action="store_true",      help="Enable HTTPS (TLS)")
    args = ap.parse_args()

    book  = OrderBook()
    store = StateStore()

    def publish(event_type, data):
        if event_type == "log":
            store.push_log(data["label"], data["body"], data["tag"])
        elif event_type == "book":
            store.push_book(data)
        elif event_type == "status":
            store.set_connected(data.get("connected", False))

    fix_conn = FIXConnection(book, publish)
    fix_conn.config["host"] = args.fix_host
    fix_conn.config["port"] = args.fix_port

    server = ThreadingHTTPServer(("0.0.0.0", args.http_port), Handler)
    server.fix_conn      = fix_conn
    server.book          = book
    server.store         = store
    server.password_hash = (
        hashlib.sha256(args.password.encode()).hexdigest() if args.password else ""
    )

    # ── TLS ──────────────────────────────────────────────────────────────────
    scheme = "http"
    if args.https:
        cert = args.cert or "fix_terminal_cert.pem"
        key  = args.key  or "fix_terminal_key.pem"
        if not os.path.exists(cert) or not os.path.exists(key):
            print("Generating self-signed TLS certificate …")
            generate_self_signed_cert(cert, key)
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.load_cert_chain(cert, key)
        server.socket = ctx.wrap_socket(server.socket, server_side=True)
        scheme = "https"

    # ── startup banner ────────────────────────────────────────────────────────
    proto = scheme.upper()
    print()
    print(f"  ╔══════════════════════════════════════════════╗")
    print(f"  ║        FIX 4.2 Web Terminal                  ║")
    print(f"  ╚══════════════════════════════════════════════╝")
    print()
    print(f"  Local access   →  {scheme}://localhost:{args.http_port}")
    print(f"  FIX server     →  {args.fix_host}:{args.fix_port}")
    print()
    if args.password:
        print(f"  Auth           →  password protected ✓")
    else:
        print(f"  Auth           →  NONE — add --password <pw> before exposing to internet!")
    if args.https:
        print(f"  TLS            →  enabled ✓  (browser will warn about self-signed cert)")
    else:
        print(f"  TLS            →  disabled — add --https before exposing to internet!")
    print()
    print(f"  To access from internet:")
    print(f"    1. Find your public IP:  curl ifconfig.me")
    print(f"    2. Forward port {args.http_port} on your router to this machine")
    print(f"    3. Open {scheme}://<your-public-ip>:{args.http_port}")
    if args.https:
        print(f"    4. Accept the self-signed cert warning in your browser")
    print()
    print("  Press Ctrl-C to stop.")
    print()

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nShutting down.")
        fix_conn.disconnect()
        server.server_close()


if __name__ == "__main__":
    main()