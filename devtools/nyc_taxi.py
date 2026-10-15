# /// script
# requires-python = ">=3.10"
# dependencies = ["duckdb==1.5.*"]
# ///
"""Demo data: NYC yellow taxi trips (NYC TLC open data), cleaned into one Parquet file per month.

    uv run devtools/nyc_taxi.py DIR FIRST_MONTH LAST_MONTH
    e.g. uv run devtools/nyc_taxi.py data/duckdb/globex/nyc_taxi 2021-08 2026-07

Downloads the monthly files into DIR/raw (skipping those already there) and writes DIR/trips/YYYY-MM.parquet
with one schema for every month: zone and borough names, readable vendor / payment / rate labels, trip
duration, and implausible trips (wrong month, negative amounts, absurd distances or durations) left out.
Source: https://www.nyc.gov/site/tlc/about/tlc-trip-record-data.page
"""

import sys
import urllib.request
from pathlib import Path

import duckdb

BASE = "https://d37ci6vzurychx.cloudfront.net"


def months(first: str, last: str):
    y, m = map(int, first.split("-"))
    while f"{y:04}-{m:02}" <= last:
        yield f"{y:04}-{m:02}"
        y, m = (y + 1, 1) if m == 12 else (y, m + 1)


def fetch(url: str, path: Path):
    if path.exists():
        return
    part = path.with_suffix(".part")
    urllib.request.urlretrieve(url, part)
    part.rename(path)


def main(out: Path, first: str, last: str):
    raw, trips = out / "raw", out / "trips"
    raw.mkdir(parents=True, exist_ok=True)
    trips.mkdir(parents=True, exist_ok=True)
    zones = raw / "taxi_zone_lookup.csv"
    fetch(f"{BASE}/misc/taxi_zone_lookup.csv", zones)
    db = duckdb.connect()
    db.execute(f"CREATE TABLE zones AS SELECT LocationID AS id, Borough AS borough, Zone AS zone FROM read_csv_auto('{zones}')")
    for month in months(first, last):
        src, dst = raw / f"yellow_tripdata_{month}.parquet", trips / f"{month}.parquet"
        if dst.exists():
            continue
        fetch(f"{BASE}/trip-data/yellow_tripdata_{month}.parquet", src)
        cols = {r[0].lower() for r in db.execute(f"DESCRIBE SELECT * FROM read_parquet('{src}')").fetchall()}
        opt = lambda c: f"CAST({c} AS DOUBLE)" if c in cols else "0.0"  # columns added over the years
        y, m = map(int, month.split("-"))
        db.execute(f"""
            COPY (
              SELECT
                t.tpep_pickup_datetime AS pickup_at,
                t.tpep_dropoff_datetime AS dropoff_at,
                CAST(hour(t.tpep_pickup_datetime) AS INTEGER) AS pickup_hour,
                dayname(t.tpep_pickup_datetime) AS pickup_weekday,
                CASE CAST(t.VendorID AS INTEGER) WHEN 1 THEN 'Creative Mobile Technologies' WHEN 2 THEN 'Curb Mobility'
                  WHEN 6 THEN 'Myle Technologies' WHEN 7 THEN 'Helix' ELSE 'Other' END AS vendor,
                coalesce(pu.borough, 'Unknown') AS pickup_borough,
                coalesce(pu.zone, 'Unknown') AS pickup_zone,
                coalesce(dz.borough, 'Unknown') AS dropoff_borough,
                coalesce(dz.zone, 'Unknown') AS dropoff_zone,
                CASE CAST(t.payment_type AS INTEGER) WHEN 0 THEN 'Flex fare' WHEN 1 THEN 'Credit card' WHEN 2 THEN 'Cash'
                  WHEN 3 THEN 'No charge' WHEN 4 THEN 'Dispute' WHEN 6 THEN 'Voided' ELSE 'Unknown' END AS payment,
                CASE CAST(t.RatecodeID AS INTEGER) WHEN 1 THEN 'Standard' WHEN 2 THEN 'JFK' WHEN 3 THEN 'Newark'
                  WHEN 4 THEN 'Nassau or Westchester' WHEN 5 THEN 'Negotiated' WHEN 6 THEN 'Group ride' ELSE 'Unknown' END AS rate,
                CAST(t.passenger_count AS INTEGER) AS passengers,
                CAST(t.trip_distance AS DOUBLE) AS distance_mi,
                round(date_diff('second', t.tpep_pickup_datetime, t.tpep_dropoff_datetime) / 60.0, 2) AS duration_min,
                CAST(t.fare_amount AS DOUBLE) AS fare,
                CAST(t.tip_amount AS DOUBLE) AS tip,
                CAST(t.tolls_amount AS DOUBLE) AS tolls,
                {opt('congestion_surcharge')} AS congestion_surcharge,
                {opt('airport_fee')} AS airport_fee,
                {opt('cbd_congestion_fee')} AS cbd_congestion_fee,
                CAST(t.total_amount AS DOUBLE) AS total
              FROM read_parquet('{src}') t
              LEFT JOIN zones pu ON pu.id = t.PULocationID
              LEFT JOIN zones dz ON dz.id = t.DOLocationID
              WHERE t.tpep_pickup_datetime >= DATE '{y:04}-{m:02}-01'
                AND t.tpep_pickup_datetime < DATE '{y:04}-{m:02}-01' + INTERVAL 1 MONTH
                AND t.tpep_dropoff_datetime > t.tpep_pickup_datetime
                AND t.tpep_dropoff_datetime < t.tpep_pickup_datetime + INTERVAL 6 HOUR
                AND t.total_amount BETWEEN 0 AND 1000
                AND t.trip_distance BETWEEN 0 AND 200
              ORDER BY pickup_at
            ) TO '{dst}.part' (FORMAT parquet, COMPRESSION zstd)""")
        Path(f"{dst}.part").rename(dst)
        n = db.execute(f"SELECT count(*) FROM read_parquet('{dst}')").fetchone()[0]
        print(f"{month}: {n:,} trips", flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(Path(sys.argv[1]), sys.argv[2], sys.argv[3])
