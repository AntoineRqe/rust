import csv
import sqlite3
import sys
from pathlib import Path

def main():
    if len(sys.argv) != 3:
        print("Usage: python csv_to_sqlite.py <input.csv> <output.db>")
        sys.exit(1)

    csv_path = Path(sys.argv[1])
    db_path = Path(sys.argv[2])

    conn = sqlite3.connect(db_path)
    cur = conn.cursor()

    # Speed optimizations for bulk import
    cur.executescript("""
        PRAGMA journal_mode = OFF;
        PRAGMA synchronous = OFF;
        PRAGMA temp_store = MEMORY;
        PRAGMA cache_size = -200000;
    """)

    # Create table
    cur.execute("""
        CREATE TABLE IF NOT EXISTS sites (
            domain TEXT PRIMARY KEY,
            category TEXT
        ) WITHOUT ROWID;
    """)

    insert_sql = """
        INSERT OR REPLACE INTO sites (domain, category)
        VALUES (?, ?)
    """

    with csv_path.open(newline="", encoding="utf-8") as f:
        reader = csv.DictReader(f, delimiter=';')

        batch = []
        batch_size = 10_000

        for row in reader:
            domain = row["domain"].strip().lower()
            category = row["llm_category_1"].strip()

            if not domain:
                continue

            batch.append((domain, category))

            if len(batch) >= batch_size:
                cur.executemany(insert_sql, batch)
                conn.commit()
                batch.clear()

        if batch:
            cur.executemany(insert_sql, batch)
            conn.commit()

    conn.close()
    print(f"Database created: {db_path}")

if __name__ == "__main__":
    main()

