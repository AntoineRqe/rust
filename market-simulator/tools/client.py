"""
FIX 4.2 Order Entry Terminal
A tkinter GUI client to send NewOrderSingle (D) messages over TCP.

Requirements: Python 3.7+  (no third-party packages needed)
Run: python fix_client_gui.py
"""

import socket
import time
import threading
import tkinter as tk
from tkinter import ttk, scrolledtext
from datetime import datetime, timezone

# ─────────────────────────────────────────────────────────────
#  FIX protocol helpers
# ─────────────────────────────────────────────────────────────
SOH = "\x01"

TAG_BEGIN_STRING  = 8
TAG_BODY_LENGTH   = 9
TAG_MSG_TYPE      = 35
TAG_SENDER_COMP   = 49
TAG_TARGET_COMP   = 56
TAG_MSG_SEQ_NUM   = 34
TAG_SENDING_TIME  = 52
TAG_CHECKSUM      = 10
TAG_CL_ORD_ID     = 11
TAG_HANDL_INST    = 21
TAG_SYMBOL        = 55
TAG_SIDE          = 54
TAG_TRANSACT_TIME = 60
TAG_ORDER_QTY     = 38
TAG_ORD_TYPE      = 40
TAG_PRICE         = 44

_seq = 0

def next_seq() -> int:
    global _seq
    _seq += 1
    return _seq

def fix_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%d-%H:%M:%S")

def fld(tag: int, val) -> str:
    return f"{tag}={val}{SOH}"

def checksum(raw: str) -> str:
    return str(sum(ord(c) for c in raw) % 256).zfill(3)

def build_new_order_single(sender, target, symbol, side, qty, price) -> bytes:
    now  = fix_now()
    oid  = f"ORD-{int(time.time()*1000)}"
    body = (
        fld(TAG_MSG_TYPE,      "D")
        + fld(TAG_SENDER_COMP, sender)
        + fld(TAG_TARGET_COMP, target)
        + fld(TAG_MSG_SEQ_NUM, next_seq())
        + fld(TAG_SENDING_TIME, now)
        + fld(TAG_CL_ORD_ID,   oid)
        + fld(TAG_HANDL_INST,  1)
        + fld(TAG_SYMBOL,      symbol)
        + fld(TAG_SIDE,        side)
        + fld(TAG_TRANSACT_TIME, now)
        + fld(TAG_ORDER_QTY,   int(qty))
        + fld(TAG_ORD_TYPE,    2)
        + fld(TAG_PRICE,       f"{price:.4f}")
    )
    begin = fld(TAG_BEGIN_STRING, "FIX.4.2")
    blen  = fld(TAG_BODY_LENGTH, len(begin) + len(body))
    raw   = begin + blen + body
    raw  += fld(TAG_CHECKSUM, checksum(raw))
    return raw.encode("ascii")

def pretty_fix(raw: bytes) -> str:
    return raw.decode("ascii", errors="replace").replace(SOH, " | ")

# ─────────────────────────────────────────────────────────────
#  GUI
# ─────────────────────────────────────────────────────────────
BG        = "#0d0f14"
PANEL     = "#13161e"
BORDER    = "#1e2330"
GREEN     = "#00e676"
RED       = "#ff3d57"
BLUE      = "#2979ff"
GOLD      = "#ffd740"
TEXT      = "#e8ecf4"
MUTED     = "#5a6070"
FONT_MONO = ("Courier New", 9)
FONT_UI   = ("Helvetica", 10)
FONT_LG   = ("Helvetica", 13, "bold")
FONT_NUM  = ("Courier New", 14, "bold")


class FIXTerminal(tk.Tk):
    def __init__(self):
        super().__init__()
        self.title("FIX Order Entry  •  4.2")
        self.configure(bg=BG)
        self.resizable(True, True)
        self.minsize(780, 580)
        self._build_ui()
        self._animate_blink()

    # ── layout ────────────────────────────────────────────────
    def _build_ui(self):
        # ── top bar ──
        top = tk.Frame(self, bg=PANEL, height=44)
        top.pack(fill=tk.X, side=tk.TOP)
        tk.Label(top, text="⬡  FIX ORDER TERMINAL", bg=PANEL, fg=GREEN,
                 font=("Courier New", 12, "bold"), padx=18).pack(side=tk.LEFT, pady=8)
        self._status_dot = tk.Label(top, text="●", bg=PANEL, fg=MUTED,
                                    font=("Courier New", 14))
        self._status_dot.pack(side=tk.RIGHT, padx=8)
        tk.Label(top, text="READY", bg=PANEL, fg=MUTED,
                 font=FONT_MONO).pack(side=tk.RIGHT)

        # ── main split ──
        pane = tk.PanedWindow(self, orient=tk.HORIZONTAL, bg=BG,
                              sashwidth=4, sashrelief=tk.FLAT)
        pane.pack(fill=tk.BOTH, expand=True, padx=0, pady=0)

        left  = self._build_order_panel(pane)
        right = self._build_log_panel(pane)
        pane.add(left,  minsize=320, width=360)
        pane.add(right, minsize=380)

    def _section(self, parent, title):
        f = tk.Frame(parent, bg=PANEL, bd=0, relief=tk.FLAT)
        tk.Label(f, text=title.upper(), bg=PANEL, fg=MUTED,
                 font=("Courier New", 8, "bold"), anchor="w", padx=0).pack(
                     fill=tk.X, pady=(10, 4))
        sep = tk.Frame(f, bg=BORDER, height=1)
        sep.pack(fill=tk.X, pady=(0, 8))
        return f

    def _labeled_entry(self, parent, label, default="", width=22, validate=None):
        row = tk.Frame(parent, bg=PANEL)
        row.pack(fill=tk.X, pady=3)
        tk.Label(row, text=label, bg=PANEL, fg=MUTED,
                 font=FONT_MONO, width=14, anchor="w").pack(side=tk.LEFT)
        var = tk.StringVar(value=default)
        vcmd = (self.register(validate), "%P") if validate else None
        e = tk.Entry(row, textvariable=var, bg="#1a1e2a", fg=TEXT,
                     insertbackground=GREEN, relief=tk.FLAT,
                     font=FONT_NUM, width=width,
                     highlightthickness=1, highlightcolor=BLUE,
                     highlightbackground=BORDER,
                     **({"validate": "key", "validatecommand": vcmd} if vcmd else {}))
        e.pack(side=tk.LEFT, ipady=5, padx=(4, 0))
        return var, e

    def _build_order_panel(self, parent):
        frame = tk.Frame(parent, bg=PANEL, padx=16, pady=10)

        # ── Connection ──
        sec = self._section(frame, "connection")
        sec.pack(fill=tk.X)
        self._host_var, _ = self._labeled_entry(sec, "Host", "127.0.0.1", width=18)
        self._port_var, _ = self._labeled_entry(sec, "Port", "9876",      width=18)
        self._send_var, _ = self._labeled_entry(sec, "SenderCompID", "CLIENT1", width=18)
        self._tgt_var,  _ = self._labeled_entry(sec, "TargetCompID", "SERVER1", width=18)

        # ── Instrument ──
        sec2 = self._section(frame, "instrument")
        sec2.pack(fill=tk.X)
        self._sym_var,  _ = self._labeled_entry(sec2, "Symbol",   "AAPL", width=18)

        # ── Order ──
        sec3 = self._section(frame, "order")
        sec3.pack(fill=tk.X)

        def is_numeric(val):
            if val == "" or val == ".": return True
            try: float(val); return True
            except: return False

        self._qty_var,   self._qty_entry   = self._labeled_entry(sec3, "Quantity",   "100",    width=18, validate=is_numeric)
        self._price_var, self._price_entry = self._labeled_entry(sec3, "Limit Price","150.00", width=18, validate=is_numeric)

        # ── Side selector ──
        side_row = tk.Frame(sec3, bg=PANEL)
        side_row.pack(fill=tk.X, pady=(10, 4))
        tk.Label(side_row, text="Side", bg=PANEL, fg=MUTED,
                 font=FONT_MONO, width=14, anchor="w").pack(side=tk.LEFT)
        self._side_var = tk.StringVar(value="BUY")
        self._btn_buy  = self._side_btn(side_row, "BUY",  "BUY",  GREEN, self._side_var)
        self._btn_sell = self._side_btn(side_row, "SELL", "SELL", RED,   self._side_var)
        self._btn_buy.pack(side=tk.LEFT, padx=(4, 2))
        self._btn_sell.pack(side=tk.LEFT, padx=2)
        self._side_var.trace_add("write", lambda *_: self._refresh_side_buttons())

        # ── Send button ──
        self._send_btn = tk.Button(
            frame, text="▶  SEND ORDER", command=self._send_order,
            bg=GREEN, fg=BG, font=("Helvetica", 12, "bold"),
            relief=tk.FLAT, cursor="hand2", pady=10,
            activebackground="#00c853", activeforeground=BG,
        )
        self._send_btn.pack(fill=tk.X, pady=(18, 4))

        # ── Reset seq ──
        tk.Button(frame, text="Reset sequence", command=self._reset_seq,
                  bg=PANEL, fg=MUTED, font=("Helvetica", 8),
                  relief=tk.FLAT, cursor="hand2",
                  activebackground=BORDER, activeforeground=TEXT
        ).pack(anchor="e", pady=(0, 6))

        return frame

    def _side_btn(self, parent, text, value, color, var):
        def select():
            var.set(value)
        b = tk.Button(parent, text=text, command=select,
                      font=("Helvetica", 10, "bold"),
                      relief=tk.FLAT, cursor="hand2",
                      width=7, pady=5)
        b._color = color
        b._value = value
        return b

    def _refresh_side_buttons(self):
        sel = self._side_var.get()
        for btn in (self._btn_buy, self._btn_sell):
            active = (btn._value == sel)
            btn.config(
                bg=btn._color if active else "#1a1e2a",
                fg=BG if active else MUTED,
                highlightthickness=0,
            )
        self._send_btn.config(
            bg=GREEN if sel == "BUY" else RED,
            text=f"▶  {'BUY' if sel == 'BUY' else 'SELL'}  ORDER",
            activebackground="#00c853" if sel == "BUY" else "#c62828",
        )

    def _build_log_panel(self, parent):
        frame = tk.Frame(parent, bg=BG, padx=12, pady=10)

        hdr = tk.Frame(frame, bg=BG)
        hdr.pack(fill=tk.X, pady=(4, 6))
        tk.Label(hdr, text="MESSAGE LOG", bg=BG, fg=MUTED,
                 font=("Courier New", 8, "bold")).pack(side=tk.LEFT)
        tk.Button(hdr, text="CLEAR", command=self._clear_log,
                  bg=BG, fg=MUTED, font=("Courier New", 8),
                  relief=tk.FLAT, cursor="hand2",
                  activebackground=BORDER, activeforeground=TEXT
        ).pack(side=tk.RIGHT)

        self._log = scrolledtext.ScrolledText(
            frame, bg="#0a0c10", fg=TEXT,
            font=FONT_MONO, relief=tk.FLAT, wrap=tk.WORD,
            insertbackground=GREEN,
            selectbackground=BLUE,
        )
        self._log.pack(fill=tk.BOTH, expand=True)

        # colour tags
        self._log.tag_config("ts",    foreground=MUTED)
        self._log.tag_config("send",  foreground=GREEN)
        self._log.tag_config("recv",  foreground=GOLD)
        self._log.tag_config("err",   foreground=RED)
        self._log.tag_config("info",  foreground=BLUE)
        self._log.tag_config("field", foreground="#80cbc4")
        self._log.configure(state=tk.DISABLED)

        self._refresh_side_buttons()
        return frame

    # ── logging ───────────────────────────────────────────────
    def _log_write(self, text, tag=""):
        self._log.configure(state=tk.NORMAL)
        self._log.insert(tk.END, text, tag)
        self._log.see(tk.END)
        self._log.configure(state=tk.DISABLED)

    def _log_line(self, label, msg, tag):
        ts = datetime.now().strftime("%H:%M:%S.%f")[:-3]
        self._log_write(f"\n[{ts}] ", "ts")
        self._log_write(f"{label}\n", tag)
        self._log_write(msg + "\n", "field")

    def _clear_log(self):
        self._log.configure(state=tk.NORMAL)
        self._log.delete("1.0", tk.END)
        self._log.configure(state=tk.DISABLED)

    # ── status dot blink ──────────────────────────────────────
    def _animate_blink(self):
        col = self._status_dot.cget("fg")
        self._status_dot.config(fg=GREEN if col == MUTED else MUTED)
        self.after(900, self._animate_blink)

    def _set_status(self, text, color):
        self._status_dot.config(fg=color)

    # ── reset ─────────────────────────────────────────────────
    def _reset_seq(self):
        global _seq
        _seq = 0
        self._log_line("INFO", "Sequence number reset to 0.", "info")

    # ── send ──────────────────────────────────────────────────
    def _send_order(self):
        try:
            host   = self._host_var.get().strip()
            port   = int(self._port_var.get().strip())
            sender = self._send_var.get().strip()
            target = self._tgt_var.get().strip()
            symbol = self._sym_var.get().strip().upper()
            qty    = float(self._qty_var.get())
            price  = float(self._price_var.get())
            side   = 1 if self._side_var.get() == "BUY" else 2
        except ValueError as exc:
            self._log_line("ERROR", f"Invalid input: {exc}", "err")
            return

        self._send_btn.config(state=tk.DISABLED, text="SENDING …")
        threading.Thread(target=self._worker,
                         args=(host, port, sender, target, symbol, side, qty, price),
                         daemon=True).start()

    def _worker(self, host, port, sender, target, symbol, side, qty, price):
        try:
            msg = build_new_order_single(sender, target, symbol, side, qty, price)
            readable = pretty_fix(msg)
            self.after(0, self._log_line, "SENT ▶", readable, "send")

            with socket.create_connection((host, port), timeout=8) as sock:
                sock.sendall(msg)
                sock.settimeout(8)
                try:
                    resp = sock.recv(4096)
                    if resp:
                        self.after(0, self._log_line, "◀ RECV",
                                   pretty_fix(resp), "recv")
                    else:
                        self.after(0, self._log_line, "INFO",
                                   "Connection closed by server.", "info")
                except socket.timeout:
                    self.after(0, self._log_line, "INFO",
                               "No response (timeout) — message delivered.", "info")

        except ConnectionRefusedError:
            self.after(0, self._log_line, "ERROR",
                       f"Connection refused at {host}:{port}", "err")
        except OSError as exc:
            self.after(0, self._log_line, "ERROR", str(exc), "err")
        finally:
            side_label = "BUY" if side == 1 else "SELL"
            self.after(0, self._send_btn.config, {
                "state": tk.NORMAL,
                "text": f"▶  {side_label}  ORDER",
            })


# ─────────────────────────────────────────────────────────────
if __name__ == "__main__":
    app = FIXTerminal()
    app.mainloop()