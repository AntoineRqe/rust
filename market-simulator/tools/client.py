"""
FIX 4.2 Order Entry + Market Data Terminal  —  tkinter edition
No third-party packages required.
Run:  python fix_client_gui.py

Layout:
  Left   — connection + order entry + market data buttons
  Centre — live order book (bids / asks + last trades)
  Right  — raw message log
"""

import socket
import time
import threading
from datetime import datetime, timezone
from collections import defaultdict
import tkinter as tk
from tkinter import scrolledtext

# ─────────────────────────────────────────────────────────────────────────────
#  FIX helpers
# ─────────────────────────────────────────────────────────────────────────────
SOH = "\x01"
_seq = 0

def next_seq():
    global _seq; _seq += 1; return _seq

def reset_seq():
    global _seq; _seq = 0

def fix_now():
    return datetime.now(timezone.utc).strftime("%Y%m%d-%H:%M:%S")

def make_cl_ord_id(prefix="ORD"):
    return f"{prefix}-{int(time.time()*1000)}"

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
    """Return {tag_int: last_value} for a raw FIX message."""
    fields = {}
    for part in raw.decode("ascii", errors="replace").split(SOH):
        if "=" in part:
            k, _, v = part.partition("=")
            try: fields[int(k)] = v
            except ValueError: pass
    return fields

def build_nos(sender, target, symbol, side, qty, price, cl_ord_id=None):
    now = fix_now(); oid = cl_ord_id or make_cl_ord_id("ORD")
    return _wrap(
        fld(35,"D")+fld(49,sender)+fld(56,target)+fld(34,next_seq())+fld(52,now)
        +fld(11,oid)+fld(21,1)+fld(55,symbol)+fld(54,side)+fld(60,now)
        +fld(38,int(qty))+fld(40,2)+fld(44,f"{price:.4f}")
    )

def build_cancel(sender, target, symbol, side, qty, orig_cl_ord_id):
    now = fix_now(); cancel_id = make_cl_ord_id("CXL")
    return _wrap(
        fld(35,"F")+fld(49,sender)+fld(56,target)+fld(34,next_seq())+fld(52,now)
        +fld(11,cancel_id)+fld(41,orig_cl_ord_id)+fld(55,symbol)+fld(54,side)
        +fld(38,int(qty))+fld(60,now)
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

def pretty(raw: bytes) -> str:
    return raw.decode("ascii", errors="replace").replace(SOH, " │ ")

# ─────────────────────────────────────────────────────────────────────────────
#  Order book model
# ─────────────────────────────────────────────────────────────────────────────
class OrderBook:
    """
    Maintains bids and asks as {price: qty} dicts.
    Updated from ExecutionReports (35=8), MD Snapshot (W) and MD Incremental (X).
    """
    MAX_TRADES = 20
    MAX_LEVELS = 8

    def __init__(self):
        self.bids   = {}   # price -> qty  (float -> float)
        self.asks   = {}
        self.trades = []   # list of (price, qty, side, ts)
        self.orders = {}   # cl_ord_id -> (side, price, qty)
        self.symbol = "—"
        self.last   = None
        self._lock  = threading.Lock()

    def _safe_float(self, v, default=0.0):
        try: return float(v)
        except: return default

    def apply(self, fields: dict):
        msg_type = fields.get(35, "")

        with self._lock:
            sym = fields.get(55)
            if sym: self.symbol = sym

            # ── ExecutionReport (8) ───────────────────────────────────────
            if msg_type == "8":
                ord_status = fields.get(39, "")
                side       = fields.get(54, "")
                cl_ord_id  = fields.get(11, "")
                orig_id    = fields.get(41, "")
                last_px    = self._safe_float(fields.get(31))
                last_qty   = self._safe_float(fields.get(32))
                price      = self._safe_float(fields.get(44) or fields.get(6))
                qty        = self._safe_float(fields.get(38))

                if last_px > 0 and last_qty > 0:
                    # It's a fill — record trade and update book
                    self.last = last_px
                    ts = datetime.now().strftime("%H:%M:%S")
                    self.trades.insert(0, (last_px, last_qty,
                                           "B" if side == "1" else "S", ts))
                    self.trades = self.trades[:self.MAX_TRADES]

                    # Remove filled qty from the resting side
                    if side == "1":    # buy hit the ask
                        self._remove_qty(self.asks, last_px, last_qty)
                    else:              # sell hit the bid
                        self._remove_qty(self.bids, last_px, last_qty)

                elif ord_status in ("0", "A"):  # New / PendingNew
                    if price > 0 and qty > 0:
                        if side == "1":
                            self.bids[price] = self.bids.get(price, 0) + qty
                        else:
                            self.asks[price] = self.asks.get(price, 0) + qty
                        if cl_ord_id:
                            self.orders[cl_ord_id] = (side, price, qty)

                elif ord_status in ("4", "C"):  # Cancelled / Expired
                    if price > 0 and qty > 0:
                        if side == "1":
                            self._remove_qty(self.bids, price, qty)
                        else:
                            self._remove_qty(self.asks, price, qty)
                        if cl_ord_id:
                            self.orders.pop(cl_ord_id, None)
                    else:
                        ref_id = orig_id or cl_ord_id
                        if ref_id and ref_id in self.orders:
                            ref_side, ref_price, ref_qty = self.orders.pop(ref_id)
                            if ref_side == "1":
                                self._remove_qty(self.bids, ref_price, ref_qty)
                            else:
                                self._remove_qty(self.asks, ref_price, ref_qty)
                        else:
                            if side == "1" and self.bids:
                                best_bid = max(self.bids.keys())
                                self.bids.pop(best_bid, None)
                            elif side == "2" and self.asks:
                                best_ask = min(self.asks.keys())
                                self.asks.pop(best_ask, None)

            # ── MD Snapshot Full Refresh (W) ──────────────────────────────
            elif msg_type == "W":
                # Re-parse raw to handle repeating groups 269/270/271
                pass  # handled via apply_md_snapshot

            # ── MD Incremental Refresh (X) ────────────────────────────────
            elif msg_type == "X":
                action     = int(fields.get(279, 0))
                entry_type = fields.get(269, "0")
                px         = self._safe_float(fields.get(270))
                qty        = self._safe_float(fields.get(271))
                book       = self.bids if entry_type == "0" else self.asks
                if action == 0:    # New
                    book[px] = book.get(px, 0) + qty
                elif action == 1:  # Change
                    if qty > 0: book[px] = qty
                    else: book.pop(px, None)
                elif action == 2:  # Delete
                    book.pop(px, None)

    def apply_md_snapshot(self, raw: bytes):
        """Parse full repeating groups from raw bytes."""
        parts = raw.decode("ascii", errors="replace").split(SOH)
        with self._lock:
            self.bids.clear()
            self.asks.clear()
            current_type = None
            current_px   = None
            for part in parts:
                if "=" not in part: continue
                k, _, v = part.partition("=")
                try: tag = int(k)
                except: continue
                if tag == 55:  self.symbol = v
                elif tag == 269: current_type = v; current_px = None
                elif tag == 270: current_px = float(v) if v else None
                elif tag == 271 and current_px is not None:
                    qty = float(v) if v else 0
                    if current_type == "0":   self.bids[current_px] = qty
                    elif current_type == "1": self.asks[current_px] = qty

    def _remove_qty(self, book, price, qty):
        if price in book:
            book[price] = max(0, book[price] - qty)
            if book[price] == 0:
                del book[price]

    def snapshot(self):
        """Return (sorted_bids, sorted_asks, trades, last_price) thread-safely."""
        with self._lock:
            bids   = sorted(self.bids.items(),  reverse=True)[:self.MAX_LEVELS]
            asks   = sorted(self.asks.items())[:self.MAX_LEVELS]
            trades = list(self.trades)
            last   = self.last
            sym    = self.symbol
        return bids, asks, trades, last, sym


# ─────────────────────────────────────────────────────────────────────────────
#  Theme
# ─────────────────────────────────────────────────────────────────────────────
BG       = "#0d0f14"
PANEL    = "#13161e"
PANEL2   = "#0f111a"
BORDER   = "#1e2330"
GREEN    = "#00e676"
RED      = "#ff3d57"
BLUE     = "#2979ff"
GOLD     = "#ffd740"
TEAL     = "#00bcd4"
TEXT     = "#e8ecf4"
MUTED    = "#5a6070"
MONO     = ("Courier New", 9)
MONO_MED = ("Courier New", 10)
MONO_LG  = ("Courier New", 11, "bold")
UI_BOLD  = ("Helvetica", 10, "bold")
UI_LG    = ("Helvetica", 12, "bold")
UI_SM    = ("Helvetica", 9)

BID_BG   = "#0a1a10"   # dark green tint for bid rows
ASK_BG   = "#1a0a0e"   # dark red tint for ask rows
BID_BAR  = "#003a18"
ASK_BAR  = "#3a0a10"


def mk_entry(parent, var, width=14):
    return tk.Entry(parent, textvariable=var, bg="#1a1e2a", fg=TEXT,
                    insertbackground=GREEN, relief=tk.FLAT, font=MONO_MED, width=width,
                    highlightthickness=1, highlightcolor=BLUE, highlightbackground=BORDER)

def flat_btn(parent, text, cmd, bg=PANEL, fg=TEXT, font=UI_BOLD, padx=10, pady=6, width=None):
    kw = dict(text=text, command=cmd, bg=bg, fg=fg, font=font, relief=tk.FLAT,
              cursor="hand2", padx=padx, pady=pady,
              activebackground=BORDER, activeforeground=TEXT)
    if width: kw["width"] = width
    return tk.Button(parent, **kw)

def sep(parent, color=BORDER):
    return tk.Frame(parent, bg=color, height=1)


# ─────────────────────────────────────────────────────────────────────────────
#  Modal dialogs
# ─────────────────────────────────────────────────────────────────────────────
class ThemedDialog(tk.Toplevel):
    def __init__(self, master, title, on_send):
        super().__init__(master)
        self.title(title); self.configure(bg=PANEL)
        self.resizable(False, False); self.grab_set()
        self._on_send = on_send; self._vars = {}
        self._build()
        self.update_idletasks()
        x = master.winfo_x() + master.winfo_width()//2  - self.winfo_width()//2
        y = master.winfo_y() + master.winfo_height()//2 - self.winfo_height()//2
        self.geometry(f"+{x}+{y}")

    def _build(self): raise NotImplementedError

    def _row(self, p, lbl, key, default="", width=20):
        f = tk.Frame(p, bg=PANEL); f.pack(fill=tk.X, pady=3)
        tk.Label(f, text=f"{lbl:<18}", bg=PANEL, fg=MUTED, font=MONO).pack(side=tk.LEFT)
        var = tk.StringVar(value=default); self._vars[key] = var
        mk_entry(f, var, width).pack(side=tk.LEFT, ipady=4, padx=(4,0))
        return var

    def _opt_row(self, p, lbl, key, options, default):
        f = tk.Frame(p, bg=PANEL); f.pack(fill=tk.X, pady=3)
        tk.Label(f, text=f"{lbl:<18}", bg=PANEL, fg=MUTED, font=MONO).pack(side=tk.LEFT)
        var = tk.StringVar(value=default); self._vars[key] = var
        om = tk.OptionMenu(f, var, *options)
        om.config(bg="#1a1e2a", fg=TEXT, font=MONO, relief=tk.FLAT,
                  activebackground=BORDER, activeforeground=TEXT,
                  highlightthickness=1, highlightbackground=BORDER, width=22)
        om["menu"].config(bg="#1a1e2a", fg=TEXT, font=MONO,
                          activebackground=BLUE, activeforeground=TEXT)
        om.pack(side=tk.LEFT, padx=(4,0)); return var

    def _footer(self, p):
        f = tk.Frame(p, bg=PANEL); f.pack(fill=tk.X, pady=(14,2))
        flat_btn(f, "CANCEL", self.destroy, bg=BORDER, fg=MUTED).pack(side=tk.RIGHT, padx=(4,0))
        flat_btn(f, "SEND", self._submit, bg=BLUE, fg=TEXT).pack(side=tk.RIGHT)

    def _submit(self):
        self._on_send({k: v.get().strip() for k, v in self._vars.items()})
        self.destroy()


class MDRequestDialog(ThemedDialog):
    def _build(self):
        p = tk.Frame(self, bg=PANEL, padx=20, pady=16); p.pack()
        tk.Label(p, text="MARKET DATA REQUEST  (V)", bg=PANEL, fg=TEAL, font=MONO_LG).pack(anchor="w", pady=(0,10))
        self._row(p, "Symbol", "symbol", "AAPL")
        self._row(p, "Market Depth", "depth", "1")
        self._footer(p)

class MDSnapshotDialog(ThemedDialog):
    def _build(self):
        p = tk.Frame(self, bg=PANEL, padx=20, pady=16); p.pack()
        tk.Label(p, text="MD SNAPSHOT FULL REFRESH  (W)", bg=PANEL, fg=TEAL, font=MONO_LG).pack(anchor="w", pady=(0,10))
        self._row(p, "Symbol", "symbol", "AAPL")
        self._row(p, "Bid",    "bid",    "149.50")
        self._row(p, "Ask",    "ask",    "150.50")
        self._row(p, "Qty",    "qty",    "100")
        self._footer(p)

class MDIncrementalDialog(ThemedDialog):
    def _build(self):
        p = tk.Frame(self, bg=PANEL, padx=20, pady=16); p.pack()
        tk.Label(p, text="MD INCREMENTAL REFRESH  (X)", bg=PANEL, fg=TEAL, font=MONO_LG).pack(anchor="w", pady=(0,10))
        self._row(p, "Symbol", "symbol", "AAPL")
        self._row(p, "Price",  "price",  "150.00")
        self._row(p, "Qty",    "qty",    "100")
        self._opt_row(p, "Update Action", "action",
                      ["0 = New","1 = Change","2 = Delete"], "0 = New")
        self._opt_row(p, "Entry Type", "entry_type",
                      ["0 = Bid","1 = Offer","2 = Trade"], "0 = Bid")
        self._footer(p)

class MDRejectDialog(ThemedDialog):
    def _build(self):
        p = tk.Frame(self, bg=PANEL, padx=20, pady=16); p.pack()
        tk.Label(p, text="MD REQUEST REJECT  (Y)", bg=PANEL, fg=RED, font=MONO_LG).pack(anchor="w", pady=(0,10))
        self._row(p, "MDReqID",        "req_id", "MDR-0")
        self._opt_row(p, "Reject Reason", "reason_code",
                      ["0 = Unknown Symbol","1 = Dup MDReqID",
                       "2 = Insuf. Bandwidth","3 = Insuf. Perms",
                       "4 = Unsup. SubReqType","5 = Unsup. MarketDepth",
                       "6 = Unsup. MDUpdateType","7 = Unsup. AggBook",
                       "8 = Unsup. MDEntryType"], "0 = Unknown Symbol")
        self._row(p, "Text (optional)", "text", "", width=24)
        self._footer(p)


# ─────────────────────────────────────────────────────────────────────────────
#  Order Book Widget
# ─────────────────────────────────────────────────────────────────────────────
class OrderBookWidget(tk.Frame):
    LEVELS    = 8
    ROW_H     = 22
    BAR_MAX_W = 120   # px — max width of the volume bar canvas

    def __init__(self, parent):
        super().__init__(parent, bg=PANEL2)
        self._build()

    def _build(self):
        # ── header ──
        hdr = tk.Frame(self, bg=PANEL2)
        hdr.pack(fill=tk.X, padx=8, pady=(10,4))
        self._sym_lbl = tk.Label(hdr, text="ORDER BOOK", bg=PANEL2,
                                 fg=TEAL, font=("Courier New", 10, "bold"))
        self._sym_lbl.pack(side=tk.LEFT)
        self._last_lbl = tk.Label(hdr, text="last: —", bg=PANEL2,
                                  fg=GOLD, font=("Courier New", 9))
        self._last_lbl.pack(side=tk.RIGHT)

        sep(self, BORDER).pack(fill=tk.X, padx=8, pady=(0,4))

        # ── column header ──
        col_hdr = tk.Frame(self, bg=PANEL2)
        col_hdr.pack(fill=tk.X, padx=8)
        for txt, anchor, w in [("QTY","e",8),("PRICE","e",10),("","w",14)]:
            tk.Label(col_hdr, text=txt, bg=PANEL2, fg=MUTED,
                     font=("Courier New", 7, "bold"),
                     width=w, anchor=anchor).pack(side=tk.LEFT, padx=1)

        # ── asks (displayed top→bottom, lowest ask at bottom) ──
        self._ask_frame = tk.Frame(self, bg=PANEL2)
        self._ask_frame.pack(fill=tk.X, padx=8)
        self._ask_rows = [self._make_row(self._ask_frame, "ask")
                          for _ in range(self.LEVELS)]

        # ── spread ──
        self._spread_frame = tk.Frame(self, bg=PANEL2, height=24)
        self._spread_frame.pack(fill=tk.X, padx=8)
        self._spread_frame.pack_propagate(False)
        self._spread_lbl = tk.Label(self._spread_frame, text="spread: —",
                                    bg=PANEL2, fg=MUTED,
                                    font=("Courier New", 8))
        self._spread_lbl.pack(expand=True)

        # ── bids ──
        self._bid_frame = tk.Frame(self, bg=PANEL2)
        self._bid_frame.pack(fill=tk.X, padx=8)
        self._bid_rows = [self._make_row(self._bid_frame, "bid")
                          for _ in range(self.LEVELS)]

        sep(self, BORDER).pack(fill=tk.X, padx=8, pady=(6,4))

        # ── last trades ──
        tk.Label(self, text="LAST TRADES", bg=PANEL2, fg=MUTED,
                 font=("Courier New", 7, "bold"),
                 anchor="w", padx=8).pack(fill=tk.X)

        self._trades_box = tk.Text(self, bg=PANEL2, fg=TEXT,
                                   font=("Courier New", 9),
                                   relief=tk.FLAT, height=6,
                                   state=tk.DISABLED)
        self._trades_box.pack(fill=tk.BOTH, expand=True, padx=8, pady=(2,8))
        self._trades_box.tag_config("buy",  foreground=GREEN)
        self._trades_box.tag_config("sell", foreground=RED)
        self._trades_box.tag_config("ts",   foreground=MUTED)

    def _make_row(self, parent, side):
        """Returns dict of label widgets for one price level."""
        bg  = BID_BG if side == "bid" else ASK_BG
        row = tk.Frame(parent, bg=bg, height=self.ROW_H)
        row.pack(fill=tk.X, pady=1)
        row.pack_propagate(False)

        # volume bar (canvas drawn behind text)
        bar = tk.Canvas(row, bg=bg, highlightthickness=0, height=self.ROW_H)
        bar.place(relx=0, rely=0, relwidth=1, relheight=1)

        qty_lbl = tk.Label(row, text="", bg=bg,
                           fg=GREEN if side == "bid" else RED,
                           font=("Courier New", 9), width=8, anchor="e")
        qty_lbl.pack(side=tk.LEFT, padx=(4,0))

        px_lbl = tk.Label(row, text="", bg=bg,
                          fg=GREEN if side == "bid" else RED,
                          font=("Courier New", 9, "bold"), width=10, anchor="e")
        px_lbl.pack(side=tk.LEFT, padx=2)

        return {"frame": row, "bar": bar, "qty": qty_lbl, "px": px_lbl,
                "side": side, "bg": bg}

    def _draw_bar(self, row_dict, fraction):
        bar   = row_dict["bar"]
        side  = row_dict["side"]
        color = BID_BAR if side == "bid" else ASK_BAR
        bar.delete("all")
        w = bar.winfo_width()
        if w < 2: return
        fill_w = int(w * min(fraction, 1.0))
        if fill_w > 0:
            bar.create_rectangle(0, 0, fill_w, self.ROW_H, fill=color, outline="")

    def refresh(self, bids, asks, trades, last, symbol):
        """Update all widgets from fresh book snapshot. Called on main thread."""
        self._sym_lbl.config(text=f"ORDER BOOK  {symbol}")
        if last is not None:
            self._last_lbl.config(text=f"last: {last:.4f}")

        # max qty for bar scaling
        all_qty = [q for _, q in bids] + [q for _, q in asks]
        max_qty = max(all_qty) if all_qty else 1

        # asks — show lowest ask at the bottom of the ask block
        padded_asks = asks[:self.LEVELS]
        padded_asks = padded_asks + [(None, None)] * (self.LEVELS - len(padded_asks))
        # reverse so lowest ask is closest to spread
        padded_asks = list(reversed(padded_asks))

        for i, row_dict in enumerate(self._ask_rows):
            px, qty = padded_asks[i]
            if px is not None:
                row_dict["px"].config(text=f"{px:.4f}")
                row_dict["qty"].config(text=f"{qty:,.0f}")
                self._draw_bar(row_dict, qty / max_qty)
            else:
                row_dict["px"].config(text="")
                row_dict["qty"].config(text="")
                row_dict["bar"].delete("all")

        # spread
        if bids and asks:
            spread = asks[0][0] - bids[0][0]
            mid    = (asks[0][0] + bids[0][0]) / 2
            self._spread_lbl.config(
                text=f"spread: {spread:.4f}   mid: {mid:.4f}")
        else:
            self._spread_lbl.config(text="spread: —")

        # bids
        padded_bids = bids[:self.LEVELS]
        padded_bids = padded_bids + [(None, None)] * (self.LEVELS - len(padded_bids))

        for i, row_dict in enumerate(self._bid_rows):
            px, qty = padded_bids[i]
            if px is not None:
                row_dict["px"].config(text=f"{px:.4f}")
                row_dict["qty"].config(text=f"{qty:,.0f}")
                self._draw_bar(row_dict, qty / max_qty)
            else:
                row_dict["px"].config(text="")
                row_dict["qty"].config(text="")
                row_dict["bar"].delete("all")

        # trades
        self._trades_box.config(state=tk.NORMAL)
        self._trades_box.delete("1.0", tk.END)
        for px, qty, side, ts in trades:
            tag  = "buy" if side == "B" else "sell"
            sym  = "▲" if side == "B" else "▼"
            self._trades_box.insert(tk.END, f"{sym} {px:>10.4f}  ", tag)
            self._trades_box.insert(tk.END, f"{qty:>8.0f}  ", "")
            self._trades_box.insert(tk.END, f"{ts}\n", "ts")
        self._trades_box.config(state=tk.DISABLED)


# ─────────────────────────────────────────────────────────────────────────────
#  Main terminal
# ─────────────────────────────────────────────────────────────────────────────
class FIXTerminal(tk.Tk):
    def __init__(self):
        super().__init__()
        self.title("FIX 4.2  •  Order Entry & Market Data Terminal")
        self.configure(bg=BG)
        self.geometry("1380x740")
        self.minsize(1100, 600)

        self._sock        = None
        self._reader_stop = threading.Event()
        self._send_lock   = threading.Lock()
        self._md_btns     = []
        self._book        = OrderBook()
        self._last_order  = None

        self._build()
        self.protocol("WM_DELETE_WINDOW", self._on_close)
        # Refresh order book display every 200ms
        self._refresh_book()

    # ── layout ───────────────────────────────────────────────────────────────
    def _build(self):
        bar = tk.Frame(self, bg=PANEL, height=40)
        bar.pack(fill=tk.X); bar.pack_propagate(False)
        tk.Label(bar, text="⬡  FIX 4.2  ORDER & MARKET DATA TERMINAL",
                 bg=PANEL, fg=GREEN, font=("Courier New", 12, "bold"),
                 padx=16).pack(side=tk.LEFT, pady=8)
        self._status_lbl = tk.Label(bar, text="● DISCONNECTED",
                                    bg=PANEL, fg=RED, font=MONO)
        self._status_lbl.pack(side=tk.RIGHT, padx=14)

        body = tk.Frame(self, bg=BG)
        body.pack(fill=tk.BOTH, expand=True)

        # Left — controls
        left = tk.Frame(body, bg=PANEL, width=300)
        left.pack(side=tk.LEFT, fill=tk.Y)
        left.pack_propagate(False)
        self._build_left(left)

        # Centre — order book
        mid = tk.Frame(body, bg=PANEL2, width=300)
        mid.pack(side=tk.LEFT, fill=tk.Y)
        mid.pack_propagate(False)
        self._ob_widget = OrderBookWidget(mid)
        self._ob_widget.pack(fill=tk.BOTH, expand=True)

        # Right — log
        right = tk.Frame(body, bg=BG)
        right.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        self._build_right(right)

    # ── left panel ───────────────────────────────────────────────────────────
    def _build_left(self, p):
        def section(title, color=MUTED):
            tk.Label(p, text=title, bg=PANEL, fg=color,
                     font=("Courier New", 8, "bold"),
                     anchor="w", padx=16).pack(fill=tk.X, pady=(14,2))
            sep(p).pack(fill=tk.X, padx=16, pady=(0,6))

        def row(lbl, default, width=13):
            f = tk.Frame(p, bg=PANEL); f.pack(fill=tk.X, padx=16, pady=2)
            tk.Label(f, text=lbl, bg=PANEL, fg=MUTED, font=MONO,
                     width=13, anchor="w").pack(side=tk.LEFT)
            var = tk.StringVar(value=default)
            e = mk_entry(f, var, width)
            e.pack(side=tk.LEFT, ipady=4, padx=(4,0))
            return var, e

        section("CONNECTION")
        self.v_host,   self.e_host   = row("Host",         "127.0.0.1")
        self.v_port,   self.e_port   = row("Port",         "9876")
        self.v_sender, self.e_sender = row("SenderCompID", "CLIENT1")
        self.v_target, self.e_target = row("TargetCompID", "SERVER1")
        self._conn_entries = [self.e_host, self.e_port, self.e_sender, self.e_target]

        self.conn_btn = flat_btn(p, "⏻  CONNECT", self._toggle_connection,
                                 bg=BLUE, fg=TEXT, font=UI_BOLD, pady=7)
        self.conn_btn.pack(fill=tk.X, padx=16, pady=(10,2))

        section("ORDER ENTRY")
        self.v_symbol, _ = row("Symbol",     "AAPL")
        self.v_qty,    _ = row("Quantity",    "100")
        self.v_price,  _ = row("Limit Price", "150.00")

        sf = tk.Frame(p, bg=PANEL); sf.pack(fill=tk.X, padx=16, pady=(6,2))
        tk.Label(sf, text="Side", bg=PANEL, fg=MUTED, font=MONO,
                 width=13, anchor="w").pack(side=tk.LEFT)
        self.v_side = "BUY"
        self.btn_buy  = tk.Button(sf, text="BUY",  command=lambda: self._set_side("BUY"),
                                  bg=GREEN, fg=BG, font=UI_BOLD, relief=tk.FLAT,
                                  cursor="hand2", width=7, pady=4,
                                  activebackground="#00c853", activeforeground=BG)
        self.btn_sell = tk.Button(sf, text="SELL", command=lambda: self._set_side("SELL"),
                                  bg="#1a1e2a", fg=MUTED, font=UI_BOLD, relief=tk.FLAT,
                                  cursor="hand2", width=7, pady=4,
                                  activebackground="#2a1e22", activeforeground=MUTED)
        self.btn_buy.pack(side=tk.LEFT, padx=(4,2))
        self.btn_sell.pack(side=tk.LEFT)

        self.send_btn = tk.Button(p, text="▶  BUY ORDER", command=self._send_order,
                                  bg=GREEN, fg=BG, font=UI_LG, relief=tk.FLAT,
                                  cursor="hand2", pady=9, state=tk.DISABLED,
                                  activebackground="#00c853", activeforeground=BG)
        self.send_btn.pack(fill=tk.X, padx=16, pady=(12,2))

        self.cancel_btn = tk.Button(p, text="✖  CANCEL LAST", command=self._send_cancel,
                        bg="#3a1010", fg=RED, font=UI_BOLD, relief=tk.FLAT,
                        cursor="hand2", pady=7, state=tk.DISABLED,
                        activebackground="#5a1a1a", activeforeground=RED)
        self.cancel_btn.pack(fill=tk.X, padx=16, pady=(4,2))

        section("MARKET DATA", TEAL)
        for txt, cmd in [
            ("V  Market Data Request",       self._md_request),
            ("W  MD Snapshot Full Refresh",  self._md_snapshot),
            ("X  MD Incremental Refresh",    self._md_incremental),
            ("Y  MD Request Reject",         self._md_reject),
        ]:
            b = flat_btn(p, txt, cmd, bg="#0e1520", fg=TEAL,
                         font=("Courier New", 9, "bold"), pady=6)
            b.pack(fill=tk.X, padx=16, pady=2)
            self._md_btns.append(b)

        sep(p).pack(fill=tk.X, padx=16, pady=(14,4))

        # clear book button
        flat_btn(p, "clear order book", self._clear_book,
                 bg=PANEL, fg=MUTED, font=UI_SM, padx=6, pady=3).pack(anchor="w", padx=16)
        flat_btn(p, "reset sequence №", self._reset_seq,
                 bg=PANEL, fg=MUTED, font=UI_SM, padx=6, pady=3).pack(anchor="e", padx=16)

        self._set_md_state(tk.DISABLED)

    # ── right panel (log) ────────────────────────────────────────────────────
    def _build_right(self, p):
        hdr = tk.Frame(p, bg=BG); hdr.pack(fill=tk.X, padx=10, pady=(10,4))
        tk.Label(hdr, text="MESSAGE LOG", bg=BG, fg=MUTED,
                 font=("Courier New", 8, "bold")).pack(side=tk.LEFT)
        flat_btn(hdr, "CLEAR", self._clear_log, bg=BG, fg=MUTED,
                 font=("Courier New", 8), padx=6, pady=2).pack(side=tk.RIGHT)

        self._log_box = scrolledtext.ScrolledText(
            p, bg="#0a0c10", fg=TEXT, font=MONO, relief=tk.FLAT,
            wrap=tk.WORD, insertbackground=GREEN,
            selectbackground=BLUE, state=tk.DISABLED)
        self._log_box.pack(fill=tk.BOTH, expand=True, padx=6, pady=(0,6))
        self._log_box.tag_config("ts",   foreground=MUTED)
        self._log_box.tag_config("send", foreground=GREEN)
        self._log_box.tag_config("feed", foreground=GOLD)
        self._log_box.tag_config("md",   foreground=TEAL)
        self._log_box.tag_config("err",  foreground=RED)
        self._log_box.tag_config("info", foreground=BLUE)
        self._log_box.tag_config("body", foreground="#9e9e9e")

    # ── order book refresh loop ───────────────────────────────────────────────
    def _refresh_book(self):
        bids, asks, trades, last, sym = self._book.snapshot()
        self._ob_widget.refresh(bids, asks, trades, last, sym)
        self.after(200, self._refresh_book)

    # ── connection ───────────────────────────────────────────────────────────
    def _toggle_connection(self):
        if self._sock is None: self._do_connect()
        else: self._do_disconnect()

    def _do_connect(self):
        host = self.v_host.get().strip()
        try: port = int(self.v_port.get().strip())
        except ValueError: self._log("ERROR", "Invalid port.", "err"); return
        self.conn_btn.config(state=tk.DISABLED, text="CONNECTING …")
        threading.Thread(target=self._connect_worker, args=(host, port), daemon=True).start()

    def _connect_worker(self, host, port):
        try:
            sock = socket.create_connection((host, port), timeout=8)
            sock.settimeout(None)
            self._sock = sock
            self._reader_stop.clear()
            threading.Thread(target=self._reader_loop, daemon=True).start()
            self.after(0, self._on_connected, host, port)
        except (OSError, ConnectionRefusedError) as exc:
            self.after(0, self._log, "ERROR", f"Cannot connect to {host}:{port} — {exc}", "err")
            self.after(0, self.conn_btn.config,
                       {"state": tk.NORMAL, "text": "⏻  CONNECT", "bg": BLUE})

    def _on_connected(self, host, port):
        self._log("INFO", f"Connected to {host}:{port}. Listening for incoming data …", "info")
        self._status_lbl.config(text="● CONNECTED", fg=GREEN)
        self.conn_btn.config(state=tk.NORMAL, text="⏼  DISCONNECT",
                             bg="#3a1010", fg=RED, activebackground="#5a1a1a")
        self.send_btn.config(state=tk.NORMAL)
        self.cancel_btn.config(state=tk.NORMAL if self._last_order else tk.DISABLED)
        self._set_md_state(tk.NORMAL)
        for e in self._conn_entries: e.config(state=tk.DISABLED)

    def _do_disconnect(self, reason=""):
        self._reader_stop.set()
        sock, self._sock = self._sock, None
        if sock:
            try: sock.shutdown(socket.SHUT_RDWR); sock.close()
            except OSError: pass
        self.after(0, self._on_disconnected, reason)

    def _on_disconnected(self, reason=""):
        self._log("INFO", "Disconnected" + (f": {reason}" if reason else "."), "info")
        self._status_lbl.config(text="● DISCONNECTED", fg=RED)
        self.conn_btn.config(state=tk.NORMAL, text="⏻  CONNECT",
                             bg=BLUE, fg=TEXT, activebackground="#1565c0")
        self.send_btn.config(state=tk.DISABLED)
        self.cancel_btn.config(state=tk.DISABLED)
        self._set_md_state(tk.DISABLED)
        for e in self._conn_entries: e.config(state=tk.NORMAL)

    def _set_md_state(self, state):
        for b in self._md_btns: b.config(state=state)

    # ── reader ───────────────────────────────────────────────────────────────
    def _reader_loop(self):
        buf = b""
        while not self._reader_stop.is_set():
            try:
                self._sock.settimeout(0.05)
                try:
                    chunk = self._sock.recv(4096)
                    if not chunk:
                        self._sock = None
                        self.after(0, self._on_disconnected, "server closed the connection")
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
                    self._handle_message(msg)
                    emitted = True

                if not emitted and len(buf) > 0:
                    next_msg = buf.find(b"8=FIX", 1)
                    if next_msg != -1:
                        msg, buf = buf[:next_msg], buf[next_msg:]
                        self._handle_message(msg)
                    elif len(buf) > 0 and self._sock and self._sock.fileno() != -1:
                        try:
                            self._sock.settimeout(0.1)
                            extra = self._sock.recv(4096)
                            if extra:
                                buf += extra
                            else:
                                self._handle_message(buf); buf = b""
                        except socket.timeout:
                            self._handle_message(buf); buf = b""
                        finally:
                            if self._sock: self._sock.settimeout(None)

            except OSError:
                if not self._reader_stop.is_set():
                    self.after(0, self._on_disconnected, "connection lost")
                return

    def _handle_message(self, raw: bytes):
        """Parse incoming message, update book, log it."""
        fields   = parse_fix(raw)
        msg_type = fields.get(35, "?")

        # Feed into order book
        if msg_type == "W":
            self._book.apply_md_snapshot(raw)
        else:
            self._book.apply(fields)

        # Pick label colour
        tag = "feed"
        labels = {
            "8": "◀ EXEC REPORT (8)",
            "W": "◀ MD SNAPSHOT (W)",
            "X": "◀ MD INCREMENTAL (X)",
            "Y": "◀ MD REJECT (Y)",
            "V": "◀ MD REQUEST (V)",
            "0": "◀ HEARTBEAT (0)",
            "1": "◀ TEST REQUEST (1)",
            "A": "◀ LOGON (A)",
            "5": "◀ LOGOUT (5)",
        }
        label = labels.get(msg_type, f"◀ MSG ({msg_type})")
        self.after(0, self._log, label, pretty(raw), tag)

    # ── transmit ─────────────────────────────────────────────────────────────
    def _transmit(self, msg, lbl, tag, btn=None):
        if self._sock is None: self._log("ERROR", "Not connected.", "err"); return
        if btn: btn.config(state=tk.DISABLED)
        def worker():
            try:
                with self._send_lock: self._sock.sendall(msg)
                self.after(0, self._log, lbl, pretty(msg), tag)
            except OSError as exc:
                self.after(0, self._log, "ERROR", f"Send failed: {exc}", "err")
                self._do_disconnect("send error")
            finally:
                if btn: self.after(0, btn.config, {"state": tk.NORMAL})
        threading.Thread(target=worker, daemon=True).start()

    # ── order ────────────────────────────────────────────────────────────────
    def _send_order(self):
        try:
            symbol = self.v_symbol.get().strip().upper()
            qty    = float(self.v_qty.get())
            price  = float(self.v_price.get())
            side   = 1 if self.v_side == "BUY" else 2
            cl_ord_id = make_cl_ord_id("ORD")
        except ValueError as exc:
            self._log("ERROR", f"Invalid input: {exc}", "err"); return
        lbl = "BUY" if side == 1 else "SELL"
        self._last_order = {
            "cl_ord_id": cl_ord_id,
            "symbol": symbol,
            "side": side,
            "qty": qty,
            "price": price,
        }
        self.cancel_btn.config(state=tk.NORMAL)
        self._transmit(
            build_nos(self.v_sender.get(), self.v_target.get(), symbol, side, qty, price, cl_ord_id),
            f"SENT ▶  ({lbl} {int(qty)} {symbol} @ {price:.4f})  ClOrdID={cl_ord_id}", "send", self.send_btn)

    def _send_cancel(self):
        if self._sock is None:
            self._log("ERROR", "Not connected.", "err")
            return
        if not self._last_order:
            self._log("INFO", "No order available to cancel.", "info")
            return

        last = self._last_order
        self._transmit(
            build_cancel(
                self.v_sender.get(),
                self.v_target.get(),
                last["symbol"],
                last["side"],
                last["qty"],
                last["cl_ord_id"],
            ),
            f"SENT ▶  CANCEL (F)  OrigClOrdID={last['cl_ord_id']}  {last['symbol']}",
            "send",
            self.cancel_btn,
        )
        self._last_order = None
        self.cancel_btn.config(state=tk.DISABLED)

    # ── market data dialogs ───────────────────────────────────────────────────
    def _md_request(self):
        MDRequestDialog(self, "Market Data Request (V)", self._on_md_request)
    def _on_md_request(self, v):
        try:
            self._transmit(build_md_request(self.v_sender.get(), self.v_target.get(),
                           v["symbol"], int(v.get("depth", 1))),
                           f"SENT ▶  MD REQUEST (V)  {v['symbol']}", "md")
        except Exception as exc: self._log("ERROR", str(exc), "err")

    def _md_snapshot(self):
        MDSnapshotDialog(self, "MD Snapshot Full Refresh (W)", self._on_md_snapshot)
    def _on_md_snapshot(self, v):
        try:
            self._transmit(build_md_snapshot(self.v_sender.get(), self.v_target.get(),
                           v["symbol"], float(v["bid"]), float(v["ask"]), float(v["qty"])),
                           f"SENT ▶  MD SNAPSHOT (W)  {v['symbol']}", "md")
        except Exception as exc: self._log("ERROR", str(exc), "err")

    def _md_incremental(self):
        MDIncrementalDialog(self, "MD Incremental Refresh (X)", self._on_md_incremental)
    def _on_md_incremental(self, v):
        try:
            action     = int(v["action"].split("=")[0].strip())
            entry_type = int(v["entry_type"].split("=")[0].strip())
            self._transmit(build_md_incremental(self.v_sender.get(), self.v_target.get(),
                           v["symbol"], action, entry_type, float(v["price"]), float(v["qty"])),
                           f"SENT ▶  MD INCREMENTAL (X)  {v['symbol']}", "md")
        except Exception as exc: self._log("ERROR", str(exc), "err")

    def _md_reject(self):
        MDRejectDialog(self, "MD Request Reject (Y)", self._on_md_reject)
    def _on_md_reject(self, v):
        try:
            code = int(v["reason_code"].split("=")[0].strip())
            self._transmit(build_md_reject(self.v_sender.get(), self.v_target.get(),
                           v["req_id"], code, v.get("text", "")),
                           f"SENT ▶  MD REJECT (Y)  req={v['req_id']}", "md")
        except Exception as exc: self._log("ERROR", str(exc), "err")

    # ── side toggle ──────────────────────────────────────────────────────────
    def _set_side(self, side):
        self.v_side = side
        if side == "BUY":
            self.btn_buy.config(bg=GREEN, fg=BG)
            self.btn_sell.config(bg="#1a1e2a", fg=MUTED)
            self.send_btn.config(bg=GREEN, activebackground="#00c853",
                                 fg=BG, text="▶  BUY ORDER")
        else:
            self.btn_sell.config(bg=RED, fg=BG)
            self.btn_buy.config(bg="#1a1e2a", fg=MUTED)
            self.send_btn.config(bg=RED, activebackground="#c62828",
                                 fg=BG, text="▶  SELL ORDER")

    # ── log ──────────────────────────────────────────────────────────────────
    def _log(self, label, body, tag="body"):
        ts = datetime.now().strftime("%H:%M:%S.%f")[:-3]
        self._log_box.config(state=tk.NORMAL)
        self._log_box.insert(tk.END, f"[{ts}] ", "ts")
        self._log_box.insert(tk.END, f"{label}\n", tag)
        self._log_box.insert(tk.END, f"{body}\n\n", "body")
        self._log_box.see(tk.END)
        self._log_box.config(state=tk.DISABLED)

    def _clear_log(self):
        self._log_box.config(state=tk.NORMAL)
        self._log_box.delete("1.0", tk.END)
        self._log_box.config(state=tk.DISABLED)

    def _clear_book(self):
        self._book = OrderBook()
        self._log("INFO", "Order book cleared.", "info")

    def _reset_seq(self):
        reset_seq(); self._log("INFO", "Sequence number reset to 0.", "info")

    def _on_close(self):
        self._do_disconnect(); self.after(200, self.destroy)


if __name__ == "__main__":
    FIXTerminal().mainloop()