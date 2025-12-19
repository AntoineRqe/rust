import csv
import sys

def read_domains(filename):
    with open(filename, newline="", encoding="utf-8") as f:
        reader = csv.reader(f, delimiter=';')
        return {
            row[0].strip().lower()
            for row in reader
            if row and row[0].strip()
        }

def main():
    if len(sys.argv) != 3:
        print("Usage: python compare_domains.py <file1.csv> <file2.csv>")
        sys.exit(1)

    file1, file2 = sys.argv[1], sys.argv[2]

    domains_1 = read_domains(file1)
    domains_2 = read_domains(file2)

    missing = domains_1 - domains_2

    with open("missing_domains.csv", "w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f, delimiter=';')
        for domain in sorted(missing):
            writer.writerow([domain])

if __name__ == "__main__":
    main()

