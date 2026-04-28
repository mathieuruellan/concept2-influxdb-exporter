# Concept2 InfluxDB Exporter

A Rust CLI tool that fetches workout data from the Concept2 online API and exports it to InfluxDB for time-series storage and Grafana visualization.

## Core Features

- Polls Concept2.com API at configurable intervals
- Stores workout data: distance, duration, calories, stroke rate, heart rate
- Tags by: workout_id, username, machine_type, workout_type, date
- Docker-ready deployment

## Tech Stack

- **Language**: Rust (2021 edition)
- **Dependencies**: reqwest, tokio, influxdb2, chrono, serde
- **Deployment**: Docker + Docker Compose

## CI/CD (Forgejo Actions)

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `build.yml` | Push/PR on main (src files) | Test Docker build |
| `release.yml` | Tag push (`v*`) | Build & push Docker image to registry |

## Project Structure

```
/src/main.rs           - Application entry point
/Dockerfile            - Docker image
/docker-compose.yml   - Local dev stack
/.env                 - Configuration template
/.forgejo/workflows/  - CI pipelines
```

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `CONCEPT2_API_TOKEN` | Yes | - | API token from concept2.com |
| `INFLUX_URL` | Yes* | - | InfluxDB URL (e.g., `http://localhost:8086`) |
| `INFLUX_ORG` | Yes* | - | InfluxDB organization |
| `INFLUX_BUCKET` | Yes* | - | InfluxDB bucket name |
| `INFLUX_TOKEN` | Yes* | - | InfluxDB API token |
| `POLL_INTERVAL_SECONDS` | No | `3600` | How often to sync (in seconds) |
| `STATE_FILE` | No | `/data/state.json` | Path to store sync state |
| `LOG_LEVEL` | No | `INFO` | Log level (DEBUG, INFO, WARN, ERROR) |

*Required if InfluxDB export is desired.

## Build & Run

The cargo build can last 20 minutes.

```bash
# Build
cargo build --release

# Run
./target/release/concept2-influxdb
```

Or with Docker:

```bash
docker build -t concept2-influxdb .
docker run -d --name concept2-influxdb \
  -e CONCEPT2_API_TOKEN=xxx \
  -e INFLUX_URL=http://influxdb:8086 \
  -e INFLUX_ORG=myorg \
  -e INFLUX_BUCKET=workouts \
  -e INFLUX_TOKEN=xxx \
  concept2-influxdb
```