import asyncio
import websockets

WS_URL = "ws://127.0.0.1:9881/ws/snapshot"  # Change host/port if needed

async def main():
    async with websockets.connect(WS_URL) as ws:
        print(f"Connected to {WS_URL}")
        try:
            async for msg in ws:
                print(f"Received: {msg}")
        except websockets.ConnectionClosed:
            print("WebSocket connection closed.")

if __name__ == "__main__":
    asyncio.run(main())
