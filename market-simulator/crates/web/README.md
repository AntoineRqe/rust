# Web Crate

The web crate provides the complete HTTP and WebSocket API for the market simulator, including a login gateway, authentication system, and real-time trading terminal.

## Architecture

The web crate is organized into specialized modules with clear separation of concerns:

```
web/src/
├── auth.rs          - Authentication, session tokens, admin checks, market filtering
├── login.rs         - HTTP handlers for login page, app, and login API
├── gateway.rs       - Login gateway server (multi-market entry point)
├── server.rs        - Market web server setup, routing, initialization
├── ws.rs            - WebSocket connection handling and business logic
├── players.rs       - Player state management, portfolios, order tracking
├── state.rs         - Event bus for broadcasting, order book snapshots
├── fix_session.rs   - FIX TCP protocol session management
└── lib.rs           - Module exports and public API
```

### Module Responsibilities

#### **auth.rs** (Authentication & Authorization)
- `SessionInfo`: Authenticated session metadata (username, admin flag)
- `generate_token()`: Create cryptographically random session tokens
- `authenticate_token()`: Validate session tokens
- `admin_password()`: Load admin credentials from environment
- `advertised_markets()`: Filter markets by public/private accessibility
- `require_admin()`: Check admin privileges and publish error messages
- `MarketInfo`: Market configuration (name, URL)

#### **login.rs** (HTTP Handlers for Market Web Server)
- `login_page_handler()`: Serve login page with token validation
- `root_handler()`: Redirect "/" to "/app"
- `app_handler()`: Serve trading terminal with market info and login gateway URL
- `api_login_handler()`: Handle login requests (admin + player authentication)
- `api_markets_handler()`: Return list of available markets

Requests to the market web server are routed here.

#### **gateway.rs** (Login Gateway Server)
- `run_login_gateway()`: Run a separate HTTP server on the entry point port
- Gateway serves as a multi-market entry point
- Proxies login requests to configured markets
- Proxies WebSocket connections to market servers
- Handles URL adaptation for public/private networks
- Runs on a dedicated thread with its own tokio runtime

#### **server.rs** (Market Web Server Setup)
- `AppState`: Shared application state (sessions, order book, player store, event bus)
- `run_web_server()`: Main entry point to start the market web server
- Router setup for all HTTP and WebSocket endpoints
- Initial order book loading from gRPC at startup
- Graceful shutdown handling

Routes:
```
GET  /           → root_handler (redirect)
GET  /login      → login_page_handler
GET  /app        → app_handler
POST /api/login  → api_login_handler
GET  /api/markets→ api_markets_handler
GET  /ws         → ws_handler (WebSocket upgrade)
```

#### **ws.rs** (WebSocket Real-Time Connection)
- `ws_handler()`: Authenticate and upgrade HTTP to WebSocket
- `handle_socket()`: Main loop for bidirectional communication
- `send_player_state()`: Send player portfolio, tokens, pending orders
- `send_order_book_snapshots()`: Send current market state
- `handle_browser_message()`: Process commands from browser client
  - Order placement (BUY/SELL)
  - Order cancellation
  - Admin commands (reset market, reset tokens, clear order book)
  - Market data requests
- Visitor counting and analytics

#### **players.rs** (Player State Management)
- `PlayerStore`: Manages player accounts, portfolios, and authentication
- `ensure_player_exists()`: Create player account if needed
- `authenticate_or_register()`: Login or auto-register with username/password
- `add_pending_order()`: Track pending orders locally
- `apply_execution()`: Update portfolio on order fill
- `reset_market_state()`: Clear all orders and state
- `reset_all_tokens()`: Reset token balances to initial values
- Portfolio tracking: holdings, cash balances, order history

#### **state.rs** (Event Broadcasting & Order Book)
- `EventBus`: Publish/subscribe for real-time events
- `WsEvent`: All event types (order updates, executions, market data, etc.)
- `OrderBookState`: In-memory snapshot of current market state
- `OrderBook`: Per-symbol order book with bids/asks
- Event publishing to all connected browsers

#### **fix_session.rs** (FIX Protocol Integration)
- `FIXSessionManager`: One persistent TCP connection per player
- `FIXSession`: Long-lived session for order processing
- Receives execution reports from the FIX engine
- Applies fills to player portfolios
- Publishes updates to browser via event bus
- Handles offline portfolio updates (session persists when browser disconnects)

## Workflow of a Message

### User Action → Order Execution

```
1. Browser sends JSON command over WebSocket
   {
     "Order": {
       "clord_id": "order-123",
       "symbol": "AAPL",
       "qty": 100.0,
       "price": 150.5,
       "side": "1"  // 1=BUY, 2=SELL
     }
   }

2. ws_handler() validates session token
   ↓ Returns 401 if invalid

3. handle_browser_message() processes the order
   - Validates sufficient tokens (BUY) or inventory (SELL)
   - Adds order to pending_orders in player state
   - Publishes "SENT" message to browser

4. build_new_order_single() creates FIX message
   - Sets sequence number
   - Formats FIX fields (ClOrdID, Symbol, Qty, Price, Side)
   - Calculates checksum

5. send_fix_over_tcp() sends to FIX engine via TCP
   - Engine routes order to market
   - Market accepts/rejects order

6. FIX engine sends execution report via TCP
   - FIX session thread receives it
   - apply_execution() updates player portfolio
   - Publishes "FILL" or "PARTIAL" event via event bus

7. Event bus broadcasts to all subscribed browsers
   - Browser WebSocket receives update
   - send_player_state() syncs portfolio
   - Browser UI updates with new holdings/tokens
```

### Admin Command → Market Reset

```
1. Admin (is_admin=true) sends reset command over WebSocket
   {
     "ClearBook": {}
   }

2. require_admin() checks privilege
   ↓ Returns error if not admin

3. gateway_login_handler() calls gRPC ResetMarket RPC
   - Market control service clears order book
   - Returns success/failure

4. On success:
   - player_store.reset_market_state() clears all orders
   - order_book.clear() removes all symbols
   - For each symbol, publish empty OrderBook event

5. Event bus broadcasts to all connected browsers
   - Browser receives empty order book
   - Browser UI clears all market data
   - Users can resume trading
```

## Login & Authentication Flow

### Login Gateway (Multi-Market Entry Point)

```
1. User visits http://GATEWAY:9875 (default)
   ↓
2. gateway_login_page_handler() serves login.html
   - Contains login form
   - Markets list hardcoded in HTML

3. User selects market and enters credentials
   ↓
4. Browser POSTs to http://GATEWAY:9875/api/login
   {
     "username": "alice",
     "password": "secret",
     "market": "market-a"
   }

5. gateway_login_handler() processes login:
   - Filter markets by public accessibility
   - Find requested market or default to first
   - For each market, try login:
     a. POST credentials to market's /api/login
     b. If success: return token + market_url + adapt URL for public network
     c. If 401: try next market
     d. If error: log and continue

6. Browser receives:
   {
     "token": "1a2b3c...",
     "username": "alice",
     "is_admin": false,
     "market_url": "https://market-a.example.com",
     "market_name": "market-a"
   }

7. Browser stores token in sessionStorage
   Redirects to market_url?token=<token>
```

### Direct Market Login (Single Market)

```
1. User visits http://MARKET:8080 directly
   ↓
2. login_page_handler() serves login.html

3. User enters credentials
   ↓
4. Browser POSTs to http://MARKET:8080/api/login
   {
     "username": "alice",
     "password": "secret"
   }

5. api_login_handler() processes login:
   - Check if admin login:
     a. Username == "admin" (case-insensitive)
     b. Password matches MARKET_SIMULATOR_ADMIN_PWD env var
     c. If both true: create SessionInfo with is_admin=true
   
   - Otherwise: authenticate_or_register()
     a. Try authenticate with credentials
     b. If fails: auto-register new player with username/password
     c. Create SessionInfo with is_admin=false

6. On success:
   - generate_token() creates random 128-bit hex token
   - sessions.insert(token, SessionInfo)
   - Return token + username + is_admin flag

7. Browser receives:
   {
     "token": "abcd1234...",
     "username": "alice",
     "is_admin": false
   }

8. Browser stores token in sessionStorage
   Navigates to /app?token=<token>
```

### Session Validation

```
1. Browser loads /app page
   - Reads token from sessionStorage
   - Uses token in WebSocket URL: /ws?token=<token>

2. WebSocket upgrade request hits ws_handler()
   ↓
3. Query parameter extraction: params.token

4. authenticate_token(&state.sessions, &token)
   - Look up token in sessions HashMap
   - Return SessionInfo if found
   - Return None if expired or invalid

5. If authenticated:
   - handle_socket() processes the connection
   - Browser receives player state immediately
   
6. If not authenticated:
   - Return 401 "Invalid or missing session token"
   - Browser shows login page
```

### Admin Access

```
Environment Variables:
- MARKET_SIMULATOR_ADMIN_PWD: Admin password (must be set)

Login Flow:
1. User enters username="admin" + password=MARKET_SIMULATOR_ADMIN_PWD
2. api_login_handler() detects admin login
3. Ensures admin player exists in database
4. Creates session with is_admin=true
5. Browser receives is_admin=true in response

Privilege Checks:
- require_admin(&state.bus, username, is_admin) called for admin-only commands
- Checks is_admin flag in WebSocket session
- If false: publishes error "This action is reserved to the admin user."
- Admin commands:
  - ResetSeq: Reset FIX sequence numbers
  - ClearBook: Clear all orders and reset market
  - ResetTokens: Reset all player token balances
```

## Market Connection Flow

### WebSocket Connection Lifecycle

```
1. Browser connects to http://MARKET:8080/ws?token=<token>

2. ws_handler():
   - Validates token via authenticate_token()
   - Upgrades HTTP to WebSocket
   - Calls handle_socket()

3. handle_socket():
   - Splits WebSocket into sender + receiver
   - Subscribes to event bus
   - Records visitor count
   - Starts FIX session for player (persistent, survives browser disconnect)

4. Main select! loop:
   ├─ rx.recv() - Events from market/FIX engine
   │  └─ Filter by recipient (only send to intended player)
   │  └─ Special handling for "feed" events (resync player state)
   │
   └─ receiver.next() - Messages from browser
      ├─ Text messages: handle_browser_message()
      ├─ Close: break loop (WebSocket closed)
      └─ Error: log and close

5. On connection established:
   - Send "Status: connected=true"
   - Send player state (tokens, pending orders, holdings)
   - Send all order book snapshots

6. Browser sends order
   ↓
7. handle_browser_message() processes (see message workflow above)

8. FIX engine publishes execution report
   ↓
9. Event bus delivers to event loop
   ↓
10. send_player_state() updates client

11. Browser disconnects
    ↓
12. FIX session continues (server-side portfolio updates)

13. Browser reconnects (same token)
    ↓
14. New WebSocket gets same FIX session
    ↓
15. Player state synced immediately
    ↓
16. Any executions that occurred offline applied
```

### FIX Protocol Integration

```
Architecture:
- One persistent TCP connection per player
- Connection managed by FIXSessionManager
- Session thread processes incoming FIX messages
- Portfolio updates sent via event bus

Flow:
1. Player connects via WebSocket
   ↓
2. FIXSessionManager.get_or_create_session(username, ...)
   - Check if session exists for username
   - If yes: reuse it
   - If no: create new TCP connection to FIX gateway

3. Session thread waits for FIX messages:
   - NewOrderSingle from handle_browser_message()
   - ExecutionReport from market
   - OrderCancelReject
   - Etc.

4. On ExecutionReport:
   - Extract fills, partial fills, rejections
   - Update player portfolio (holdings, cash)
   - Publish event via bus
   - Browser receives update

5. Benefits:
   - Portfolio updates persist even if browser disconnects
   - No lost executions
   - Can rejoin session with existing token
   - Trades continue processing offline
```

### Order Book State Management

```
Snapshot-based Architecture:
- In-memory OrderBookState at server
- Client receives full snapshots on connect
- Client receives incremental updates as events arrive

Flow:
1. Server startup:
   - Load pending orders from gRPC via load_initial_order_book()
   - Populate OrderBookState

2. Browser connects:
   - send_order_book_snapshots() sends all symbols + bids/asks
   - Client renders full order book

3. Market feed updates arrive:
   - Event bus publishes OrderBook event
   - Browser receives updated bids/asks
   - Client re-renders

4. Player executes order:
   - Order added to order book (server-side)
   - OrderBook event published
   - All browsers see order appear

5. Order fills:
   - ExecutionReport received
   - Order removed from book
   - OrderBook event published
   - Browsers see fill + remove order

Synchronization:
- Event bus ensures all clients see same state
- Late arrivals get full snapshot
- No message loss (broadcast channel)
- Event log-based recovery possible
```

## Key Concepts

### Session Token
- 128-bit random hex string (32 characters)
- Stored in-memory HashMap: token → SessionInfo
- Validated on every WebSocket connection
- Contains username + admin flag
- Session expires when server restarts (in-memory store)

### AppState
Shared mutable state wrapped in Arc for thread-safe access:
```rust
pub struct AppState {
    pub bus: EventBus,                    // Event broadcasting
    pub sessions: Arc<Mutex<HashMap>>,    // Active sessions
    pub player_store: PlayerStore,        // Player accounts & portfolios
    pub fix_session_manager: FIXSessionManager,  // Persistent FIX connections
    pub order_book: Arc<Mutex<OrderBookState>>, // Current market state
    pub active_visitors: Arc<AtomicUsize>,      // Connected browsers count
    pub total_visitors: Arc<AtomicUsize>,       // All-time connections count
    pub grpc_addr: String,                       // Market control service
    pub known_markets: Vec<MarketInfo>,          // Market configuration
}
```

### Event Bus
Tokio broadcast channel for real-time updates:
- Pub/sub pattern
- All WebSocket connections subscribe
- Events filtered per recipient (optional)
- Late subscribers get backlog (default 128 events)

### Visitor Analytics
- `active_visitors`: Current connected count (atomic)
- `total_visitors`: All-time count (atomic)
- Updated on connect/disconnect
- Published to all browsers in real-time
- Useful for server load monitoring

## Environment Variables

| Variable | Purpose | Example |
|----------|---------|---------|
| `MARKET_SIMULATOR_ADMIN_PWD` | Admin login password | `super-secret` |
| `MARKET_SIM_PUBLIC_MARKETS_ONLY` | Filter non-public URLs | `1` (true) |
| `LOGIN_GATEWAY_URL` | Client redirect after login | `http://market.example.com:8080` |
| `DATABASE_URL` | Postgres connection for players | `postgres://...` |

## Client Connection Flow (Step-by-Step)

When a browser client connects to the market, this is exactly what happens:

### Step 1: Initial Page Load
```
Browser visits http://market:8080
         ↓
    GET /
         ↓
  root_handler()
         ↓
  302 Redirect to /app
```

### Step 2: Load Trading App
```
Browser follows redirect to GET /app
         ↓
   app_handler()
         ↓
   Load index.html template
   Inject variables:
   - {{MARKET_NAME}}
   - {{LOGIN_GATEWAY_URL}}
   - {{MARKETS_JSON}}
         ↓
   200 OK + HTML
         ↓
Browser renders HTML
JavaScript executes
```

### Step 3: Browser JS Initialization
```
JavaScript checks sessionStorage
    ↓
    ├─ Token exists?
    │   ├─ YES → Continue to WebSocket
    │   └─ NO  → Redirect to /login
    │
    └─ Token exists
        ↓
    Initiate WebSocket connection
    GET /ws?token=a1b2c3d4e5f6...
```

### Step 4: WebSocket Upgrade & Token Validation
```
Browser sends:
  GET /ws?token=ABC123
         ↓
  ws_handler()
         ↓
  Extract token: "ABC123"
         ↓
  Lock sessions HashMap
         ↓
  Look up token
         ↓
     ├─ Found → SessionInfo { username: "alice", is_admin: false }
     └─ Not found → Return 401 Unauthorized
         ↓
  If valid: Upgrade HTTP → WebSocket
         ↓
  101 Switching Protocols
         ↓
  ✅ WebSocket established
```

### Step 5: Initialize WebSocket Connection

Once connected, `handle_socket()` executes:

```
handle_socket() starts
    ↓
├─ Split socket into sender & receiver
├─ Subscribe to event bus (broadcast channel)
├─ Update visitor count (atomic increment)
├─ Create/get FIX session for player
├─ Publish "Status: connected=true"
├─ Send PlayerState:
│  {
│    username: "alice",
│    tokens: 10000.0,
│    pending_orders: [..],
│    holdings: { "AAPL": HoldingSummary{..} },
│    is_admin: false,
│    visitor_count: 5,
│    ..
│  }
│
└─ Send OrderBook snapshots:
   For each symbol:
   {
     symbol: "AAPL",
     bids: [{ order_id: "1", price: 150.5, qty: 100 }, ..],
     asks: [{ order_id: "2", price: 150.6, qty: 50 }, ..],
     timestamp_ms: 1234567890
   }
         ↓
   Client UI fully initialized ✅
```

### Step 6: Real-Time Bidirectional Communication

Main event loop runs with `tokio::select!` (concurrent):

```
┌─────────────────────────────────────────────────┐
│   STREAM A: Market → Browser (Event Bus)        │
├─────────────────────────────────────────────────┤
│ FIX Engine publishes event                      │
│     ↓                                            │
│ Event bus broadcasts to all subscribers         │
│     ↓                                            │
│ Filter by recipient (skip if targeted elsewhere)│
│     ↓                                            │
│ Serialize to JSON                               │
│     ↓                                            │
│ Send over WebSocket to browser                  │
│                                                  │
│ Examples:                                        │
│ - ExecutionReport (order filled)                │
│ - Feed (market data update)                     │
│ - FixMessage (info/errors)                      │
│ - Status (connection state)                     │
└─────────────────────────────────────────────────┘

        ⬅️  ⬆️  ⬇️  ➡️

┌─────────────────────────────────────────────────┐
│   STREAM B: Browser → Market (WebSocket)        │
├─────────────────────────────────────────────────┤
│ Browser sends JSON command                      │
│     ↓                                            │
│ handle_browser_message() parses it              │
│     ↓                                            │
│ Validate:                                        │
│ - Check auth tokens                             │
│ - Check inventory (for sells)                   │
│ - Check admin flag (for admin commands)         │
│     ↓                                            │
│ Build FIX message                               │
│     ↓                                            │
│ Send over TCP to FIX engine                     │
│     ↓                                            │
│ Publish response event to bus                   │
│                                                  │
│ Supported commands:                             │
│ - Order (place new order)                       │
│ - Cancel (cancel order)                         │
│ - MdRequest (subscribe to market data)          │
│ - ResetTokens (admin: reset player funds)       │
│ - ClearBook (admin: reset order book)           │
│ - ResetSeq (admin: reset FIX sequence)          │
└─────────────────────────────────────────────────┘
```

### Step 7: Disconnect & Persistence

When browser closes connection:

```
WebSocket closes
    ↓
Exit event loop
    ↓
Decrement active_visitors
    ↓
Publish new VisitorCount event
    ↓
✅ FIX session persists!
  (Player portfolio continues updating)
  (Offline orders can be processed)
```

### Complete Timing Diagram

```
TIME    CLIENT          NETWORK         SERVER
────────────────────────────────────────────────────────────
  t0   User opens
       http://market:8080
            ├─────── GET / ──────────>  root_handler()
       <────── 302 Redirect ─────────────┤
            │
            ├─────── GET /app ────────>  app_handler()
       <───── 200 + HTML template ──────┤
            │
            JS renders & checks token
            │
       ├─────── GET /ws?token=ABC ──>  ws_handler()
       <────── 101 Switching ──────────┤
            │                           validate token
  t1        │          ✅ Upgraded      │
            │                           handle_socket()
            │                           subscribe to bus
            │                           get FIX session
       <── PlayerState + OrderBooks ──┤
            │                       
            │ UI initialized         
            │                           
  t2   User places order ──────────────> handle_browser_message()
       │                                validate
       │                                build FIX
       │                                send to FIX engine
       <──── "Order SENT" event ────────┤
            │ 
  t3        │                           FIX processes
            │                           │
       <─── ExecutionReport event ─────┤ broadcast to all
            │                           
            │ Order filled!
            │
  t4   User closes browser
       ────── WebSocket close ─────────> handle_socket() done
            │                           FIX session persists
```

### Key Implementation Details

- **Token validation**: HashMap lookup O(1)
- **Event subscription**: Lock-free broadcast channel from Tokio
- **Visitor count**: Atomic counter with relaxed memory ordering
- **Player state**: Cloned from Arc<Mutex> (cheap copy)
- **Order matching**: FIFO by time_priority, price-time priority on passive side
- **WebSocket split**: Separate sender/receiver for concurrent I/O
- **Admin checks**: `require_admin()` publishes denial event to user only
- **Session persistence**: HashMap survives browser disconnects

## Deployment

### Single Market (Direct Access)
```bash
# Run web server on port 8080
web::run_web_server(
    bus, fix_addr, grpc_addr,
    "0.0.0.0", 8080,  // Listen on all interfaces, port 8080
    database_url,
    known_markets,
    shutdown, order_book,
    core_id
)

# Users visit http://market.example.com:8080
```

### Multi-Market (Gateway + Individual Markets)
```bash
# Gateway runs on port 9875 (entry point)
web::run_login_gateway(markets, "0.0.0.0", 9875)

# Each market runs on port 8080+N
web::run_web_server(...)  // market 1 on :8081
web::run_web_server(...)  // market 2 on :8082
# etc.

# Users visit http://gateway.example.com:9875
# Gateway proxies to appropriate market
```

## Testing

```bash
# Unit tests
cargo test -p web

# Build with all features
cargo build -p web --release

# Check for issues
cargo check -p web
cargo clippy -p web
```

## Performance Considerations

- **WebSocket subscriptions**: All clients listen to broadcast channel
- **Order book snapshots**: Cloned from shared state (Arc<Mutex>)
- **FIX sessions**: One TCP connection per player (persistent)
- **Session map**: HashMap lookup O(1) per connection
- **Event bus**: Tokio broadcast is lock-free for subscribers
- **Visitor counters**: AtomicUsize with relaxed ordering

## Future Enhancements

- Session persistence (Redis/database)
- API rate limiting
- Audit logging for all trades
- WebSocket message batching
- Order book L3 optimization
- Market data snapshot caching
