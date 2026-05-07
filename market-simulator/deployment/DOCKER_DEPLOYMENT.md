# Docker Deployment Guide

## Quick Start

```bash
# 1. Build release binaries locally (uses your Rust toolchain)
./build-release.sh

# 2. Build and start all services
docker-compose build
docker-compose up -d

# 3. Check logs
docker-compose logs -f

# 4. Stop all services
docker-compose down
```

## Quick Access

Once running:
- **Gateway (Login):** http://localhost:9860
- **NASDAQ:** http://localhost:9870
- **NYSE:** http://localhost:9885

## Services

| Service | Port | Purpose |
|---------|------|---------|
| **Gateway** | 9860 | Login page & market selector |
| **NASDAQ Web** | 9870 | Trading terminal (web) |
| **NASDAQ FIX** | 9871 | FIX protocol (TCP) |
| **NASDAQ gRPC** | 50051 | Order book (gRPC) |
| **NASDAQ Proxy** | 9891 | Market data proxy |
| **NYSE Web** | 9885 | Trading terminal (web) |
| **NYSE FIX** | 9884 | FIX protocol (TCP) |
| **NYSE gRPC** | 50052 | Order book (gRPC) |
| **NYSE Proxy** | 9892 | Market data proxy |
| **Players Service** | 50053 | gRPC (Player Service) |
| **PostgreSQL** | 5433 | Database |

## Configuration

### Environment Variables

Edit `docker-compose.yml` to customize:

```yaml
environment:
  DATABASE_URL_MARKET_SIMULATOR: postgresql://user:password@postgres:5432/market_simulator
  PLAYER_SERVICE_PORT: 50052
  RUST_LOG: info  # Use 'debug' for more verbose logging
```

### Database

- **Host:** postgres (or localhost:5433 from host machine)
- **User:** market_user
- **Password:** market_password
- **Database:** market_simulator

Default data persists in `postgres_data` volume.

**Note:** Docker PostgreSQL runs on port 5433 to avoid conflicts with local PostgreSQL on 5432.

## Usage

### First Time Setup

```bash
# Start services
docker-compose up -d

# Wait for health checks to pass
docker-compose ps

# Access the application
# Open browser to http://localhost:9860
```

### Access Services

**From Host Machine:**
- Gateway: http://localhost:9860
- NASDAQ Market: http://localhost:9870
- NYSE Market: http://localhost:9885
- NASDAQ FIX (TCP): localhost:9871
- NYSE FIX (TCP): localhost:9884
- Database: localhost:5433
- Order Book gRPC: localhost:50051 (NASDAQ), localhost:50052 (NYSE)
- Players Service: localhost:50053

**Between Containers (Internal):**
- Players Service: players-service:50053
- PostgreSQL: postgres:5432

## Troubleshooting

### Check Service Status
```bash
docker-compose ps
```

### View Logs
```bash
# All services
docker-compose logs

# Specific service
docker-compose logs players-service
docker-compose logs market-simulator
docker-compose logs postgres
```

### Rebuild Images
```bash
# Force rebuild (don't use cache)
docker-compose build --no-cache

# Then restart
docker-compose up -d
```

### Clean Everything
```bash
# Stop all containers
docker-compose down

# Remove volumes (delete database data)
docker-compose down -v

# Remove images
docker image rm market-simulator-app market-simulator-players postgres:16-alpine
```

### Access Database
```bash
# From host
psql -h localhost -U market_user -d market_simulator

# From container
docker-compose exec postgres psql -U market_user -d market_simulator
```

## Performance Tuning

### Increase Resources
Edit `docker-compose.yml`:

```yaml
services:
  market-simulator:
    deploy:
      resources:
        limits:
          cpus: '2'
          memory: 2G
    ports: [...]
```

### Enable Debug Logging
```bash
RUST_LOG=debug docker-compose up
```

## Deployment to Production

### Using Docker Swarm
```bash
docker stack deploy -c docker-compose.yml market-simulator
```

### Using Kubernetes
Convert docker-compose to Kubernetes manifests:
```bash
kompose convert -f docker-compose.yml
```

### Environment Files
Create `.env` file:
```
POSTGRES_USER=market_user
POSTGRES_PASSWORD=secure_password_here
DATABASE_URL_MARKET_SIMULATOR=postgresql://market_user:secure_password_here@postgres:5432/market_simulator
RUST_LOG=info
```

Then use in docker-compose:
```yaml
env_file: .env
```

## Scaling

### Run Multiple Player Service Instances
```yaml
services:
  players-service-1:
    # ... existing config
    ports:
      - "50052:50052"

  players-service-2:
    # ... copy and change port
    ports:
      - "50053:50052"
```

### Load Balance with Nginx
See `nginx.conf.example` for a sample load balancer configuration.

## Health Checks

Services include health checks:
- PostgreSQL: pg_isready every 5s
- Players Service: Starts after DB is healthy
- Market Simulator: Starts after all dependencies ready

Check status:
```bash
docker-compose ps
```

## Support

For issues:
1. Check logs: `docker-compose logs`
2. Verify health: `docker-compose ps`
3. Check ports: `docker-compose port [service]`
4. Inspect network: `docker network inspect market-simulator_market-network`
