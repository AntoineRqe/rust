#!/bin/bash
# Usage: ./init_markets_pg.sh <db_user>
# Creates databases: market_simulator, market_nasdaq, market_nyse
# Grants all privileges on each to the specified user

set -e

if [ -z "$1" ]; then
  echo "Usage: $0 <db_user>"
  exit 1
fi
DB_USER="$1"

for DB in market_simulator market_nasdaq market_nyse; do
  echo "Creating database $DB..."
  createdb "$DB"
  echo "Granting all privileges on $DB to $DB_USER..."
  psql -d "$DB" -c "GRANT ALL PRIVILEGES ON DATABASE $DB TO $DB_USER;"
  psql -d "$DB" -c "GRANT ALL ON SCHEMA public TO $DB_USER;"
  psql -d "$DB" -c "ALTER SCHEMA public OWNER TO $DB_USER;"
done

echo "All databases created and privileges granted."
