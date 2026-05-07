# Docker Deployment - Setup Instructions

## Quick Start

From the **repository root** (not the deployment folder):

```bash
cd /home/antoine/rust/market-simulator

# 1. Build binaries (one-time, if not done already)
./build-release.sh

# 2. Build Docker images
docker-compose -f deployment/docker-compose.yml build

# 3. Start all services
docker-compose -f deployment/docker-compose.yml up -d

# 4. Verify services
docker-compose -f deployment/docker-compose.yml ps

# 5. Test gateway is responding
curl http://localhost:9860
```

## Why Run from Repository Root?

The `docker-compose.yml` in `deployment/` uses:
```yaml
build:
  context: ..          # Go up to repo root
  dockerfile: deployment/Dockerfile.gateway
```

This means:
- **Context** = `/home/antoine/rust/market-simulator/` (repo root)
- **Dockerfile** = `deployment/Dockerfile.gateway`
- **COPY** commands look in context: `COPY target/release/gateway` → finds `/target/release/gateway`

If you run from `deployment/`:
```bash
cd deployment
docker-compose build  # ❌ WILL FAIL - context becomes deployment/
```

## Services

| Service | Port | Purpose |
|---------|------|---------|
| **PostgreSQL** | 5433 | Shared database |
| **Players Service** | 50053 | gRPC player store (internal) |
| **Gateway** | 9860 | Login proxy (entry point) |
| **NASDAQ Market** | 9870 | Web UI, 9871 TCP, 50051 gRPC, 9891 proxy |
| **NYSE Market** | 9885 | Web UI, 9884 TCP, 50052 gRPC, 9892 proxy |

## Common Commands

```bash
# View logs
docker-compose -f deployment/docker-compose.yml logs -f gateway
docker-compose -f deployment/docker-compose.yml logs -f market-nasdaq

# Stop services
docker-compose -f deployment/docker-compose.yml down

# Remove everything (including volumes/data)
docker-compose -f deployment/docker-compose.yml down -v

# Rebuild images (no cache)
docker-compose -f deployment/docker-compose.yml build --no-cache

# Scale a service
docker-compose -f deployment/docker-compose.yml up -d --scale market-nasdaq=3
```

## Troubleshooting

### "context path does not exist" or "COPY failed"
**Solution**: Run from repo root, not `deployment/` folder.
```bash
cd /home/antoine/rust/market-simulator  # ✅ Correct
# NOT:
cd deployment  # ❌ Wrong
```

### "Database connection refused"
**Solution**: Postgres needs time to start. Wait 5-10 seconds and check:
```bash
docker-compose -f deployment/docker-compose.yml logs postgres
```

### "Port already in use"
**Solution**: Stop existing containers:
```bash
docker-compose -f deployment/docker-compose.yml down
# Or kill the process:
lsof -i :9860  # Find process on port 9860
kill -9 <PID>
```

### Build is slow
**Solution**: This is normal. First build takes 30-60 seconds (apt-get downloads packages).
- Subsequent builds are cached and ~5-10 seconds.
- If still slow, clear cache: `docker builder prune`

## Architecture Notes

The `deployment/` folder contains:
- `Dockerfile.gateway` - Builds gateway, market-runner, and market-simulator images
- `Dockerfile.players` - Builds players-server image
- `docker-compose.yml` - Orchestrates all 5 services
- `.dockerignore` - Copy of root (for reference)

The actual binaries and config come from **repo root**:
- `target/release/*` - Pre-built binaries
- `crates/config/*` - Configuration files

## Performance

| Task | Time |
|------|------|
| First Docker build | 30-60 seconds |
| Rebuild (cached) | 5-10 seconds |
| Market startup | 3-5 seconds |
| Full system ready | ~15 seconds |

## Next Steps

See `PER_MARKET_DEPLOYMENT.md` for:
- Adding new markets
- Kubernetes deployment
- Multi-region setup
- Performance tuning

See `DOCKER_DEPLOYMENT.md` for:
- Detailed architecture documentation
- Network configuration
- Database setup
