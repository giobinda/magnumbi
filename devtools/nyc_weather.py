# /// script
# requires-python = ">=3.10"
# dependencies = ["duckdb==1.5.*"]
# ///
"""Demo data: hourly weather in each NYC taxi borough, to join with the taxi trips.

    uv run devtools/nyc_weather.py DIR FIRST_DAY LAST_DAY
    e.g. uv run devtools/nyc_weather.py data/duckdb/globex/nyc_taxi 2021-08-01 2026-07-31

Fetches Open-Meteo's historical weather (ERA5 reanalysis, ~9 km cells; free, no key; CC BY 4.0, see
https://open-meteo.com/en/docs/historical-weather-api) for one point per borough of the taxi zone lookup
(Manhattan, Brooklyn, Queens, Bronx, Staten Island, and EWR = Newark airport) into DIR/raw/weather_*.json
(skipping those already there), and writes DIR/weather/hourly.parquet: one row per borough and hour.

Join key: `borough` + `observed_at`, an hour in New York local time like the taxi times, so
DATE_TRUNC('hour', pickup_at) = observed_at AND pickup_borough = borough. Amounts (precipitation, rain,
snowfall) are totals over the hour before `observed_at` + 1 h, i.e. during that hour. On the autumn
daylight-saving change the repeated hour is kept once.
"""

import json
import sys
import time
import urllib.parse
import urllib.request
from datetime import date, timedelta
from pathlib import Path

import duckdb

BOROUGHS = {  # a point in each; the nearest ERA5 cell is used
    "Manhattan": (40.7831, -73.9712),
    "Brooklyn": (40.6782, -73.9442),
    "Queens": (40.7282, -73.7949),
    "Bronx": (40.8448, -73.8648),
    "Staten Island": (40.5795, -74.1502),
    "EWR": (40.6895, -74.1745),
}
HOURLY = [
    "temperature_2m", "apparent_temperature", "relative_humidity_2m", "precipitation", "rain", "snowfall",
    "snow_depth", "cloud_cover", "wind_speed_10m", "wind_gusts_10m", "weather_code",
]
# WMO weather interpretation codes: (description, condition group)
CODES = {
    0: ("Clear sky", "Clear"), 1: ("Mainly clear", "Clear"), 2: ("Partly cloudy", "Cloudy"), 3: ("Overcast", "Cloudy"),
    45: ("Fog", "Fog"), 48: ("Depositing rime fog", "Fog"),
    51: ("Light drizzle", "Drizzle"), 53: ("Moderate drizzle", "Drizzle"), 55: ("Dense drizzle", "Drizzle"),
    56: ("Light freezing drizzle", "Drizzle"), 57: ("Dense freezing drizzle", "Drizzle"),
    61: ("Slight rain", "Rain"), 63: ("Moderate rain", "Rain"), 65: ("Heavy rain", "Rain"),
    66: ("Light freezing rain", "Rain"), 67: ("Heavy freezing rain", "Rain"),
    71: ("Slight snowfall", "Snow"), 73: ("Moderate snowfall", "Snow"), 75: ("Heavy snowfall", "Snow"),
    77: ("Snow grains", "Snow"),
    80: ("Slight rain showers", "Rain"), 81: ("Moderate rain showers", "Rain"), 82: ("Violent rain showers", "Rain"),
    85: ("Slight snow showers", "Snow"), 86: ("Heavy snow showers", "Snow"),
    95: ("Thunderstorm", "Thunderstorm"), 96: ("Thunderstorm with slight hail", "Thunderstorm"),
    99: ("Thunderstorm with heavy hail", "Thunderstorm"),
}


def fetch(first: date, last: date, path: Path):
    if path.exists():
        return
    params = {
        "latitude": ",".join(str(lat) for lat, _ in BOROUGHS.values()),
        "longitude": ",".join(str(lon) for _, lon in BOROUGHS.values()),
        "start_date": first.isoformat(),
        "end_date": last.isoformat(),
        "hourly": ",".join(HOURLY),
        "models": "era5_seamless",
        "cell_selection": "nearest",
        "timezone": "America/New_York",
    }
    url = "https://archive-api.open-meteo.com/v1/archive?" + urllib.parse.urlencode(params)
    for attempt in range(4):
        try:
            with urllib.request.urlopen(url, timeout=180) as r:
                data = json.load(r)
            break
        except (OSError, ValueError) as e:  # timeouts, throttling (HTTP 429), cut-off answers
            if attempt == 3:
                raise
            print(f"  {first} to {last}: {e}; retrying", flush=True)
            time.sleep(30 * (attempt + 1))
    if not isinstance(data, list) or len(data) != len(BOROUGHS):
        sys.exit(f"unexpected answer: {str(data)[:300]}")
    # one row per borough and hour, as JSON lines (DuckDB reads them below)
    with open(f"{path}.part", "w") as out:
        for borough, d in zip(BOROUGHS, data):
            h = d["hourly"]
            for i, t in enumerate(h["time"]):
                out.write(json.dumps({"borough": borough, "time": t, **{k: h[k][i] for k in HOURLY}}) + "\n")
    Path(f"{path}.part").rename(path)


def main(out: Path, first: str, last: str):
    raw, weather = out / "raw", out / "weather"
    raw.mkdir(parents=True, exist_ok=True)
    weather.mkdir(parents=True, exist_ok=True)
    start, end = date.fromisoformat(first), date.fromisoformat(last)
    # a quarter per request: bigger requests can take minutes on the free API
    have = [p.stem.split("_")[1:] for p in raw.glob("weather_*.jsonl")]
    for year in range(start.year, end.year + 1):
        for q in range(4):
            a = max(start, date(year, 3 * q + 1, 1))
            b = min(end, date(year + (q == 3), 1 if q == 3 else 3 * q + 4, 1) - timedelta(days=1))
            if a > b or any(lo <= a.isoformat() and b.isoformat() <= hi for lo, hi in have):
                continue
            fetch(a, b, raw / f"weather_{a}_{b}.jsonl")
            print(f"{a} to {b}: fetched", flush=True)
    db = duckdb.connect()
    db.execute("CREATE TABLE codes (code INTEGER, weather VARCHAR, condition VARCHAR)")
    db.executemany("INSERT INTO codes VALUES (?, ?, ?)", [(c, w, g) for c, (w, g) in CODES.items()])
    dst = weather / "hourly.parquet"
    db.execute(f"""
        COPY (
          SELECT
            w.borough,
            CAST(w.time AS TIMESTAMP) AS observed_at,
            CAST(CAST(w.time AS TIMESTAMP) AS DATE) AS observed_on,
            CAST(w.temperature_2m AS DOUBLE) AS temperature_c,
            CAST(w.apparent_temperature AS DOUBLE) AS feels_like_c,
            CAST(w.relative_humidity_2m AS INTEGER) AS humidity_pct,
            CAST(w.precipitation AS DOUBLE) AS precipitation_mm,
            CAST(w.rain AS DOUBLE) AS rain_mm,
            CAST(w.snowfall AS DOUBLE) AS snowfall_cm,
            CAST(w.snow_depth AS DOUBLE) AS snow_depth_m,
            CAST(w.cloud_cover AS INTEGER) AS cloud_cover_pct,
            CAST(w.wind_speed_10m AS DOUBLE) AS wind_speed_kmh,
            CAST(w.wind_gusts_10m AS DOUBLE) AS wind_gusts_kmh,
            CAST(w.weather_code AS INTEGER) AS weather_code,
            coalesce(c.weather, 'Unknown') AS weather,
            coalesce(c.condition, 'Unknown') AS condition
          FROM read_json('{raw}/weather_*.jsonl', format = 'newline_delimited') w
          LEFT JOIN codes c ON c.code = w.weather_code
          WHERE w.temperature_2m IS NOT NULL
          QUALIFY row_number() OVER (PARTITION BY w.borough, w.time) = 1
          ORDER BY observed_at, borough
        ) TO '{dst}.part' (FORMAT parquet, COMPRESSION zstd)""")
    Path(f"{dst}.part").rename(dst)
    n, lo, hi = db.execute(f"SELECT count(*), min(observed_at), max(observed_at) FROM read_parquet('{dst}')").fetchone()
    print(f"{dst}: {n:,} hourly rows, {lo} to {hi}")
    for row in db.execute(f"SELECT condition, count(*) FROM read_parquet('{dst}') GROUP BY 1 ORDER BY 2 DESC").fetchall():
        print(f"  {row[0]}: {row[1]:,}")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(Path(sys.argv[1]), sys.argv[2], sys.argv[3])
