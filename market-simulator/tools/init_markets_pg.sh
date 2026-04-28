# Check for POSTGRES_USER environment variable
if [ -z "$POSTGRES_USER" ]; then
  echo "Error: POSTGRES_USER environment variable is not set."
  exit 1
fi

#!/bin/bash
# Usage: ./init_markets_pg.sh
# Creates databases: market_simulator, market_nasdaq, market_nyse
# Grants all privileges on each to the specified user

set -e

DB_USER="$POSTGRES_USER"

for DB in market_simulator market_nasdaq market_nyse; do
  echo "Creating database $DB as postgres..."
  createdb "$DB"
  echo "Granting all privileges on $DB to $DB_USER..."
  psql -d "$DB" -c "GRANT ALL PRIVILEGES ON DATABASE $DB TO $DB_USER;"
  psql -d "$DB" -c "GRANT ALL ON SCHEMA public TO $DB_USER;"
  psql -d "$DB" -c "ALTER SCHEMA public OWNER TO $DB_USER;"
done

echo "All databases created and privileges granted."
