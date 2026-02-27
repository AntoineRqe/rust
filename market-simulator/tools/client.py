"""
FIX 4.2 Single Limit Order Client
Connects to a FIX server at 127.0.0.1:9876 and sends a NewOrderSingle (D) message.

Usage:
    python fix_client.py [--symbol AAPL] [--side 1] [--qty 100] [--price 150.00]

Side: 1=Buy, 2=Sell
"""

import socket
import time
import argparse
from datetime import datetime, timezone

# ── FIX field delimiter (SOH = \x01) ──────────────────────────────────────────
SOH = "\x01"

# ── FIX field tag constants ────────────────────────────────────────────────────
TAG_BEGIN_STRING     = 8
TAG_BODY_LENGTH      = 9
TAG_MSG_TYPE         = 35
TAG_SENDER_COMP_ID   = 49
TAG_TARGET_COMP_ID   = 56
TAG_MSG_SEQ_NUM      = 34
TAG_SENDING_TIME     = 52
TAG_CHECKSUM         = 10

# NewOrderSingle (D) specific tags
TAG_CL_ORD_ID        = 11
TAG_HANDL_INST       = 21
TAG_SYMBOL           = 55
TAG_SIDE             = 54
TAG_TRANSACT_TIME    = 60
TAG_ORDER_QTY        = 38
TAG_ORD_TYPE         = 40
TAG_PRICE            = 44


def fix_utcnow() -> str:
    """Return current UTC time in FIX timestamp format: YYYYMMDD-HH:MM:SS."""
    return datetime.now(timezone.utc).strftime("%Y%m%d-%H:%M:%S")


def cl_ord_id() -> str:
    """Generate a unique client order ID."""
    return f"ORD-{int(time.time() * 1000)}"


def build_field(tag: int, value) -> str:
    return f"{tag}={value}{SOH}"


def checksum(body: str) -> str:
    """Calculate FIX checksum: sum of ASCII values mod 256, zero-padded to 3 digits."""
    total = sum(ord(c) for c in body)
    return str(total % 256).zfill(3)


def build_new_order_single(
    sender: str,
    target: str,
    seq_num: int,
    symbol: str,
    side: int,
    qty: float,
    price: float,
) -> bytes:
    """
    Build a FIX 4.2 NewOrderSingle (MsgType=D) message.

    Returns the raw bytes ready to be sent over TCP.
    """
    now = fix_utcnow()

    # ── Body fields (everything between header delimiters and checksum) ────────
    body_fields = (
        build_field(TAG_MSG_TYPE, "D")
        + build_field(TAG_SENDER_COMP_ID, sender)
        + build_field(TAG_TARGET_COMP_ID, target)
        + build_field(TAG_MSG_SEQ_NUM, seq_num)
        + build_field(TAG_SENDING_TIME, now)
        + build_field(TAG_CL_ORD_ID, cl_ord_id())
        + build_field(TAG_HANDL_INST, 1)          # 1 = Automated, no broker
        + build_field(TAG_SYMBOL, symbol)
        + build_field(TAG_SIDE, side)              # 1=Buy, 2=Sell
        + build_field(TAG_TRANSACT_TIME, now)
        + build_field(TAG_ORDER_QTY, int(qty))
        + build_field(TAG_ORD_TYPE, 2)             # 2 = Limit
        + build_field(TAG_PRICE, f"{price:.2f}")
    )

    begin_string = build_field(TAG_BEGIN_STRING, "FIX.4.2")
    body_length  = len(begin_string) + len(body_fields)  # BodyLength counts from tag 35
    # Per spec, BodyLength (9) counts bytes starting from MsgType (35) up to but not
    # including the checksum delimiter.  BeginString (8) is NOT counted.
    body_length_field = build_field(TAG_BODY_LENGTH, body_length)

    raw = begin_string + body_length_field + body_fields
    chk = checksum(raw)
    raw += build_field(TAG_CHECKSUM, chk)

    return raw.encode("ascii")


def send_fix_order(
    host: str,
    port: int,
    sender: str,
    target: str,
    symbol: str,
    side: int,
    qty: float,
    price: float,
    timeout: float = 10.0,
):
    """Connect to the FIX server, send a NewOrderSingle, and print the response."""

    msg = build_new_order_single(
        sender=sender,
        target=target,
        seq_num=1,
        symbol=symbol,
        side=side,
        qty=qty,
        price=price,
    )

    print(f"\n{'='*60}")
    print(f"Connecting to {host}:{port} …")
    print(f"{'='*60}")
    print("Outgoing FIX message (SOH shown as '|'):")
    print(msg.decode("ascii").replace(SOH, "|"))
    print(f"{'='*60}\n")

    with socket.create_connection((host, port), timeout=timeout) as sock:
        sock.sendall(msg)
        print("Message sent. Waiting for response …\n")

        # Receive up to 4 KB of response
        sock.settimeout(timeout)
        try:
            response = sock.recv(4096)
            if response:
                print("Server response (SOH shown as '|'):")
                print(response.decode("ascii", errors="replace").replace(SOH, "|"))
            else:
                print("Server closed the connection without a response.")
        except socket.timeout:
            print("No response received within timeout — server may not send one.")

    print("\nDone.")


# ── CLI ────────────────────────────────────────────────────────────────────────
def main():
    parser = argparse.ArgumentParser(description="Send a FIX 4.2 limit order.")
    parser.add_argument("--host",   default="127.0.0.1", help="FIX server host")
    parser.add_argument("--port",   default=9876, type=int, help="FIX server port")
    parser.add_argument("--sender", default="CLIENT1",   help="SenderCompID")
    parser.add_argument("--target", default="SERVER1",   help="TargetCompID")
    parser.add_argument("--symbol", default="AAPL",      help="Instrument symbol")
    parser.add_argument("--side",   default=1, type=int, help="1=Buy 2=Sell")
    parser.add_argument("--qty",    default=100.0, type=float, help="Order quantity")
    parser.add_argument("--price",  default=150.00, type=float, help="Limit price")
    args = parser.parse_args()

    send_fix_order(
        host=args.host,
        port=args.port,
        sender=args.sender,
        target=args.target,
        symbol=args.symbol,
        side=args.side,
        qty=args.qty,
        price=args.price,
    )


if __name__ == "__main__":
    main()