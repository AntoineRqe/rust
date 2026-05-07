# Per-Market Docker Deployment Architecture

## Overview

The Market Simulator now supports a distributed, per-market container architecture. Each market runs in its own Docker container while sharing centralized database and player service infrastructure.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Docker Network                          │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │  Gateway     │  │NASDAQ Market │  │ NYSE Market  │      │
│  │  9860        │  │ 9870-9871    │  │ 9884-9885    │      │
│  │              │  │ 50051, 9891  │  │ 50052, 9892  │      │
│  └────────┬─────┘  └──────┬───────┘  └──────┬───────┘      │
│           │                │                 │               │
│           └────────────────┼─────────────────┘               │
│                            │                                 │
│                    ┌───────▼────────┐                        │
│                    │ Players Service│                        │
│                    │   gRPC :50053  │                        │
│                    └────────┬────────┘                        │
│                             │                                │
│                    ┌────────▼────────┐                       │
│                    │   PostgreSQL    │                       │
│                    │   :5433         │                       │
│                    └─────────────────┘                       │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

## Services

### 1. Gateway (`gateway` binary)
- **Port**: 9860
- **Role**: Single entry point for all markets
- **Function**: Login proxy that routes to individual markets
- **Config**: `crates/config/docker.json`

### 2. NASDAQ Market (`market-runner --config nasdaq.json`)
- **Web UI**: Port 9870
- **FIX Server**: Port 9871
- **gRPC Control**: Port 50051
- **Market Proxy**: Port 9891
- **Config**: `crates/config/markets/nasdaq.json`

### 3. NYSE Market (`market-runner --config nyse.json`)
- **Web UI**: Port 9885
- **FIX Server**: Port 9884
- **gRPC Control**: Port 50052
- **Market Proxy**: Port 9892
- **Config**: `crates/config/markets/nyse.json`

### 4. Players Service (shared)
- **Port**: 50053 (internal)
- **Role**: Centralized player/token management
- **Config**: `crates/config/docker.json`

### 5. PostgreSQL (shared)
- **Port**: 5433
- **Role**: Single database for all markets
- **Database**: `market_simulator`

## Building and Running

### Prerequisites
```bash
# Ensure you have Rust installed
cargo --version

# Ensure Docker is installed
docker --version
docker-compose --version
```

### Build Process
```bash
# Build all binaries to target/release/
./build-release.sh

# Build Docker images
docker-compose build

# Start all services
docker-compose up -d

# Verify all services are running
docker-compose ps
```

### Verify Deployment
```bash
# Check login page
curl http://localhost:9860

# Check NASDAQ is running
curl http://localhost:9870

# Check NYSE is running
curl http://localhost:9885

# View logs
docker-compose logs gateway
docker-compose logs market-nasdaq
docker-compose logs market-nyse
```

## Managing Markets

### Start All Services
```bash
docker-compose up -d
```

### Stop All Services
```bash
docker-compose down
```

### Restart a Single Market
```bash
# Restart NASDAQ without affecting NYSE
docker-compose restart market-nasdaq

# Or stop and start
docker-compose stop market-nasdaq
docker-compose start market-nasdaq
```

### View Market Logs
```bash
# Real-time logs
docker-compose logs -f market-nasdaq

# Last 50 lines
docker-compose logs market-nasdaq --tail=50
```

### Scale a Market (for testing/load simulation)
```bash
# Scale NASDAQ to 2 instances (not typically used in production)
docker-compose up -d --scale market-nasdaq=2
```

## Adding a New Market

### 1. Create Market Config
Create `crates/config/markets/new-market.json`:
```json
{
  "ring_buffer_size": 1024,
  "players_service": {
    "grpc": {
      "ip": "players-service",
      "port": 50053
    },
    "database_url_env": "DATABASE_URL_MARKET_SIMULATOR",
    "core": 13
  },
  "market": {
    "name": "NEW_MARKET",
    "database_url_env": "DATABASE_URL_NEW_MARKET",
    "web": {
      "ip": "127.0.0.1",
      "port": 9900
    },
    "tcp": {
      "ip": "0.0.0.0",
      "port": 9901
    },
    "grpc": {
      "ip": "0.0.0.0",
      "port": 50054
    },
    "proxy": {
      "ip": "0.0.0.0",
      "port": 9902
    },
    "market_feed_multicast": {
      "ip": "239.0.0.1",
      "port": 9005
    },
    "snapshot_multicast": {
      "ip": "239.0.0.1",
      "port": 9006
    },
    "snapshot": {
      "max_depth": 5,
      "update_interval_ms": 100000
    },
    "core_mapping": {
      "order_book_core": 0,
      "fix_inbound_core": 1,
      "fix_outbound_core": 2,
      "execution_report_core": 3,
      "market_feed_core": 4,
      "db_core": 5,
      "web_core": 6,
      "tcp_core": 7,
      "global_core": 8,
      "snapshot_core": 9,
      "market_feed_multicast_core": 10,
      "snapshot_multicast_core": 11,
      "market_data_proxy_core": 12
    }
  }
}
```

### 2. Add Service to docker-compose.yml
```yaml
  market-newmarket:
    build:
      context: .
      dockerfile: Dockerfile.gateway
    container_name: market-simulator-newmarket
    environment:
      DATABASE_URL_NEW_MARKET: postgresql://market_user:market_password@postgres:5432/market_simulator
      DATABASE_URL_MARKET_SIMULATOR: postgresql://market_user:market_password@postgres:5432/market_simulator
      RUST_LOG: info
      PLAYER_SERVICE_ADDR: http://players-service:50053
    ports:
      - "9900:9900" # Web UI
      - "9901:9901" # TCP FIX
      - "50054:50054" # gRPC order book
      - "9902:9902" # Market data proxy
    depends_on:
      postgres:
        condition: service_healthy
      players-service:
        condition: service_started
    networks:
      - market-network
    restart: unless-stopped
    command: ["market-runner", "--config", "crates/config/markets/new-market.json"]
```

### 3. Update Gateway Config (docker.json)
Add entry to `markets` array in `crates/config/docker.json`:
```json
{
  "name": "NEW_MARKET",
  "database_url_env": "DATABASE_URL_NEW_MARKET",
  "web": {
    "ip": "market-newmarket",
    "port": 9900
  },
  ...
}
```

### 4. Build and Deploy
```bash
docker-compose build
docker-compose up -d
```

## Environment Variables

Each market service supports these environment variables:

- `DATABASE_URL_{MARKET}`: PostgreSQL connection string for market data
- `DATABASE_URL_MARKET_SIMULATOR`: Shared database URL for players
- `PLAYER_SERVICE_ADDR`: Address of gRPC player service (docker-compose: `http://players-service:50053`)
- `RUST_LOG`: Logging level (default: `info`)

## Troubleshooting

### Markets fail to start
Check logs:
```bash
docker-compose logs market-nasdaq
```

Common issues:
- Missing `ring_buffer_size` in market config → Add `"ring_buffer_size": 1024`
- Port conflicts → Ensure ports are unique across markets
- Database connection → Verify `DATABASE_URL_*` environment variables
- Players service unreachable → Verify players-service is running and healthy

### WebSocket connection fails
- Ensure market is fully started (check logs for "Web terminal" message)
- Verify correct port (9870 for NASDAQ, 9885 for NYSE)
- Check firewall rules allow connections to ports

### High memory usage
Each market instance loads the entire order book into memory. For production:
- Monitor memory usage per market
- Consider sharding order books by symbol if needed
- Adjust `ring_buffer_size` in config (default: 1024 entries)

## Performance Characteristics

### Startup Time
- NASDAQ/NYSE: ~3-5 seconds from container start to accepting connections
- Full cluster (all services): ~10-15 seconds

### Memory Usage (per market)
- Market-simulator binary: ~100-150 MB
- Order book buffer (1024 entries): ~50 MB
- Total per market: ~200 MB

### Network Usage
- Multicast streams: Market feed (UDP) + Snapshot feed (UDP)
- gRPC: Control requests (negligible)
- WebSocket: Real-time order updates (per client)

## Kubernetes Deployment

To deploy to Kubernetes:

1. **Services**:
   - Gateway Deployment (replicas: 1)
   - Market-NASDAQ Deployment (replicas: 1-N)
   - Market-NYSE Deployment (replicas: 1-N)
   - Players Service StatefulSet (replicas: 1)
   - PostgreSQL StatefulSet (replicas: 1)

2. **ConfigMaps**:
   - docker.json (gateway config)
   - market configs (per market)

3. **Services**:
   - gateway-service (LoadBalancer for port 9860)
   - market-nasdaq-service (ClusterIP for port 9870)
   - market-nyse-service (ClusterIP for port 9885)
   - players-service (ClusterIP for port 50053)
   - postgres-service (ClusterIP for port 5432)

4. **PersistentVolumes**:
   - PostgreSQL data volume

Example Kubernetes manifests can be generated from docker-compose.yml using `kompose convert`.

## Future Enhancements

- [ ] Dynamic service discovery (avoid hardcoded docker.json IPs)
- [ ] Health check endpoints for markets
- [ ] Metrics export per market (Prometheus)
- [ ] Multi-region support with replicated player service
- [ ] Configuration hot-reloading
- [ ] Automated market startup/shutdown based on schedule

## Related Documentation

- [Docker Deployment Guide](DOCKER_DEPLOYMENT.md)
- [Configuration Reference](crates/config/README.md)
- [Architecture Documentation](README.md)
