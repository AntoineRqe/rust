#!/bin/bash

# Market Simulator Docker Helper Script

set -e

COMMAND=${1:-help}

case "$COMMAND" in
  up)
    echo "Starting Market Simulator with Docker..."
    docker-compose up -d
    echo ""
    echo "Services started!"
    echo ""
    echo "📊 Access the application:"
    echo "  Gateway (Login):     http://localhost:3000"
    echo "  Market 1 (NASDAQ):   http://localhost:3001"
    echo "  Market 2 (NYSE):     http://localhost:3002"
    echo "  Market 3 (AMEX):     http://localhost:3003"
    echo ""
    echo "Check status: docker-compose ps"
    echo "View logs:   docker-compose logs -f"
    ;;

  down)
    echo "Stopping Market Simulator..."
    docker-compose down
    echo "Services stopped"
    ;;

  logs)
    SERVICE=${2:-}
    if [ -z "$SERVICE" ]; then
      docker-compose logs -f
    else
      docker-compose logs -f "$SERVICE"
    fi
    ;;

  ps)
    echo "Service Status:"
    docker-compose ps
    ;;

  build)
    echo "Building Docker images..."
    docker-compose build --no-cache
    echo "Images built"
    ;;

  rebuild)
    echo "Rebuilding everything..."
    docker-compose down -v
    docker-compose build --no-cache
    docker-compose up -d
    echo "Everything rebuilt and started"
    ;;

  shell)
    SERVICE=${2:-postgres}
    echo "🐚 Opening shell in $SERVICE..."
    docker-compose exec "$SERVICE" sh
    ;;

  db)
    echo "Connecting to database..."
    docker-compose exec postgres psql -U market_user -d market_simulator
    ;;

  clean)
    echo "Cleaning up..."
    docker-compose down -v
    echo "All data deleted (volumes removed)"
    ;;

  status)
    echo "📊 System Status:"
    docker-compose ps
    echo ""
    echo "Network: market-simulator_market-network"
    docker network inspect market-simulator_market-network 2>/dev/null || echo "Network not created yet"
    ;;

  *)
    echo "Market Simulator Docker Helper"
    echo ""
    echo "Usage: ./docker.sh [command] [args]"
    echo ""
    echo "Commands:"
    echo "  up         - Start all services"
    echo "  down       - Stop all services"
    echo "  logs       - View logs (optionally: logs [service])"
    echo "  ps         - Show service status"
    echo "  build      - Build Docker images"
    echo "  rebuild    - Rebuild and restart everything"
    echo "  shell      - Open shell in service (default: postgres)"
    echo "  db         - Connect to PostgreSQL"
    echo "  clean      - Stop and delete all data"
    echo "  status     - Show system status"
    echo "  help       - Show this help message"
    echo ""
    echo "Examples:"
    echo "  ./docker.sh up                    # Start everything"
    echo "  ./docker.sh logs market-simulator # View app logs"
    echo "  ./docker.sh shell players-service # Shell into player service"
    echo "  ./docker.sh db                    # Connect to database"
    ;;
esac
