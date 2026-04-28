use chrono::{DateTime, TimeZone, Utc};
use futures::stream;
use influxdb2::Client as InfluxClient;
use influxdb2::models::DataPoint;
use log::{debug, error, info};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::panic;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

// Panic handler for better error reporting in Docker
fn panic_handler(info: &panic::PanicHookInfo) {
    eprintln!("PANIC: {}", info);
    std::process::exit(1);
}

// ============================================================================
// Configuration
// ============================================================================

#[derive(Clone)]
struct Config {
    concept2_api_base: String,
    api_token: String,
    poll_interval_seconds: i64,
    state_file: PathBuf,
    log_level: String,
    influx_url: Option<String>,
    influx_org: Option<String>,
    influx_bucket: Option<String>,
    influx_token: Option<String>,
}

impl Config {
    fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let state_file = PathBuf::from(
            std::env::var("CONCEPT2_STATE_FILE").unwrap_or_else(|_| "/data/state.json".to_string()),
        );

        Ok(Config {
            concept2_api_base: "https://log.concept2.com/api".to_string(),
            api_token: std::env::var("CONCEPT2_API_TOKEN")?,
            poll_interval_seconds: std::env::var("CONCEPT2_POLL_INTERVAL_SECONDS")
                .unwrap_or_else(|_| "3600".to_string())
                .parse::<i64>()?,
            state_file,
            log_level: std::env::var("CONCEPT2_LOG_LEVEL").unwrap_or_else(|_| "INFO".to_string()),
            influx_url: std::env::var("CONCEPT2_INFLUX_URL").ok(),
            influx_org: std::env::var("CONCEPT2_INFLUX_ORG").ok(),
            influx_bucket: std::env::var("CONCEPT2_INFLUX_BUCKET").ok(),
            influx_token: std::env::var("CONCEPT2_INFLUX_TOKEN").ok(),
        })
    }
}

// ============================================================================
// Types
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SyncState {
    last_synced_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkoutMetrics {
    username: String,
    machine_type: String,
    workout_type: String,
    date: String,
    distance: f64,
    duration: f64,
    calories: f64,
    spm: f64,
    hr: f64,
    timestamp: f64,
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    data: T,
    meta: Option<ApiMeta>,
}

#[derive(Debug, Deserialize)]
struct ApiMeta {
    pagination: Option<PaginationMeta>,
}

#[derive(Debug, Deserialize)]
struct PaginationMeta {
    current_page: u32,
    total_pages: u32,
}

#[derive(Debug, Deserialize)]
struct UserData {
    id: u64,
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkoutResult {
    id: u64,
    #[serde(rename = "type")]
    machine_type: Option<String>,
    workout_type: Option<String>,
    date: Option<String>,
    date_utc: Option<String>,
    distance: Option<f64>,
    time: Option<i64>,
    #[serde(rename = "calories_total")]
    calories: Option<f64>,
    stroke_rate: Option<f64>,
    heart_rate: Option<HeartRateData>,
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HeartRateData {
    average: Option<f64>,
}

// ============================================================================
// State Helpers
// ============================================================================

fn load_state(config: &Config) -> SyncState {
    match fs::read_to_string(&config.state_file) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(state) => state,
            Err(_) => SyncState {
                last_synced_at: None,
            },
        },
        Err(_) => SyncState {
            last_synced_at: None,
        },
    }
}

fn save_state(config: &Config, state: &SyncState) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(config.state_file.parent().unwrap())?;
    let json = serde_json::to_string_pretty(state)?;
    fs::write(&config.state_file, json)?;
    Ok(())
}

impl Config {
    fn influxdb_enabled(&self) -> bool {
        self.influx_url.is_some()
            && self.influx_org.is_some()
            && self.influx_bucket.is_some()
            && self.influx_token.is_some()
    }
}

async fn write_to_influxdb(
    client: &InfluxClient,
    bucket: &str,
    workout: &WorkoutMetrics,
    workout_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let timestamp_ns = if workout.timestamp > 0.0 {
        let secs = workout.timestamp as i64;
        Utc.timestamp_opt(secs, 0).unwrap().timestamp_nanos_opt().unwrap_or(0)
    } else {
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    };

    let point = DataPoint::builder("workouts")
        .tag("workout_id", workout_id)
        .tag("username", &workout.username)
        .tag("machine_type", &workout.machine_type)
        .tag("workout_type", &workout.workout_type)
        .tag("date", &workout.date)
        .field("distance_meters", workout.distance)
        .field("duration_seconds", workout.duration)
        .field("calories", workout.calories)
        .field("stroke_rate_avg", workout.spm)
        .field("heart_rate_avg", workout.hr)
        .timestamp(timestamp_ns)
        .build()?;

    client.write(bucket, stream::iter(vec![point])).await?;

    info!("Wrote workout {} to InfluxDB", workout_id);
    Ok(())
}

// ============================================================================
// Concept2 API
// ============================================================================

fn api_headers(config: &Config) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {}", config.api_token)
            .parse()
            .unwrap(),
    );
    headers.insert(
        reqwest::header::ACCEPT,
        "application/json".parse().unwrap(),
    );
    headers
}

async fn fetch_user_id(config: &Config) -> Result<(String, String), Box<dyn std::error::Error>> {
    let client = Client::new();
    let url = format!("{}/users/me", config.concept2_api_base);
    let resp = client
        .get(&url)
        .headers(api_headers(config))
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    resp.error_for_status_ref()?;
    let raw = resp.text().await?;
    debug!("fetch_user_id raw response: {}", raw);
    let body: ApiResponse<UserData> = serde_json::from_str(&raw)?;
    let user_id = body.data.id.to_string();
    let username = body.data.username.unwrap_or_else(|| user_id.clone());
    Ok((user_id, username))
}

async fn fetch_results_since(
    config: &Config,
    user_id: &str,
    updated_after: Option<String>,
) -> Result<Vec<WorkoutResult>, Box<dyn std::error::Error>> {
    let client = Client::new();
    let mut results = Vec::new();
    let mut page = 1;
    const PAGE_SIZE: u32 = 250;

    loop {
        let mut params = vec![("per_page", PAGE_SIZE.to_string())];
        if let Some(ref date) = updated_after {
            params.push(("updated_after", date.clone()));
        }
        params.push(("page", page.to_string()));

        let url = format!(
            "{}/users/{}/results",
            config.concept2_api_base, user_id
        );
        let resp = client
            .get(&url)
            .headers(api_headers(config))
            .query(&params)
            .timeout(Duration::from_secs(30))
            .send()
            .await?;

        resp.error_for_status_ref()?;
        let raw = resp.text().await?;
        debug!("fetch_results_since page={} raw response: {}", page, raw);
        let body: ApiResponse<Vec<WorkoutResult>> = serde_json::from_str(&raw)?;
        let page_data = body.data;

        if let Some(meta) = &body.meta {
            if let Some(pagination) = &meta.pagination {
                debug!(
                    "Page {}/{} — got {} result(s)",
                    pagination.current_page,
                    pagination.total_pages,
                    page_data.len()
                );
                results.extend(page_data);
                if pagination.current_page >= pagination.total_pages {
                    debug!(
                        "Last page reached ({} >= {}), stopping",
                        pagination.current_page, pagination.total_pages
                    );
                    break;
                }
                page += 1;
            }
        } else {
            results.extend(page_data);
            break;
        }
    }

    Ok(results)
}

// ============================================================================
// Duration & Timestamp Parsing
// ============================================================================

fn parse_duration(raw: &WorkoutResult) -> f64 {
    // API returns time in tenths of seconds
    if let Some(tenths) = raw.time {
        return tenths as f64 / 10.0;
    }
    0.0
}

fn parse_timestamp(date_str: &str) -> f64 {
    if date_str.is_empty() {
        return 0.0;
    }

    let formats = [
        "%Y-%m-%dT%H:%M:%S%z",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d",
    ];

    for fmt in &formats {
        let to_parse = if date_str.len() < 19 {
            date_str
        } else {
            &date_str[..19]
        };

        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(to_parse, fmt) {
            return DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc).timestamp() as f64;
        }
    }
    0.0
}

// ============================================================================
// Workout Conversion
// ============================================================================

fn workout_to_metrics(workout: &WorkoutResult, username: &str) -> WorkoutMetrics {
    let machine_type = workout.machine_type.as_deref().unwrap_or("unknown").to_string();
    let workout_type = workout.workout_type.as_deref().unwrap_or("unknown").to_string();
    let date = workout
        .date
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(10)
        .collect::<String>();

    let distance = (workout.distance.unwrap_or(0.0) as i64).max(0);
    let duration = parse_duration(workout) as i64;
    let calories = (workout.calories.unwrap_or(0.0) as i64).max(0);
    let spm = (workout.stroke_rate.unwrap_or(0.0) as i64).max(0);
    let hr = workout
        .heart_rate
        .as_ref()
        .and_then(|hr| hr.average)
        .unwrap_or(0.0) as i64;
    let ts = parse_timestamp(
        workout
            .date_utc
            .as_deref()
            .or(workout.date.as_deref())
            .unwrap_or(""),
    ) as i64;

    WorkoutMetrics {
        username: username.to_string(),
        machine_type,
        workout_type,
        date,
        distance: distance as f64,
        duration: duration as f64,
        calories: calories as f64,
        spm: spm as f64,
        hr: hr as f64,
        timestamp: ts as f64,
    }
}

// ============================================================================
// Polling Loop
// ============================================================================

async fn sync_once(
    config: &Config,
    user_id: &str,
    username: &str,
    state: SyncState,
    influx: Option<(&InfluxClient, &str)>,
) -> Result<SyncState, Box<dyn std::error::Error>> {
    let updated_after = state.last_synced_at.clone();
    info!(
        "Fetching workouts updated_after={}",
        updated_after.as_deref().unwrap_or("beginning")
    );

    let workouts = fetch_results_since(config, user_id, updated_after).await?;
    info!("Fetched {} workout(s)", workouts.len());

    let mut new_count = 0;
    let mut newest_date = state.last_synced_at;

    for workout in workouts {
        let wid = workout.id.to_string();
        let workout_metrics = workout_to_metrics(&workout, username);

        let write_result = if let Some((client, bucket)) = influx {
            write_to_influxdb(client, bucket, &workout_metrics, &wid).await
        } else {
            Ok(())
        };

        if let Err(e) = write_result {
            error!("Failed to write workout {} to InfluxDB: {}", wid, e);
            continue;
        }

        new_count += 1;

        let w_date = workout
            .updated_at
            .as_ref()
            .or(workout.date.as_ref())
            .cloned();
        if let Some(w_date_str) = w_date {
            if newest_date.is_none() || w_date_str > newest_date.as_ref().unwrap().clone() {
                newest_date = Some(w_date_str);
            }
        }
    }

    info!("Recorded {} new workout(s)", new_count);

    Ok(SyncState {
        last_synced_at: newest_date,
    })
}

async fn run_sync(
    config: &Config,
    user_id: &str,
    username: &str,
    influx: Option<(InfluxClient, &str)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let influx_ref = influx.as_ref().map(|(c, b)| (c, *b));
    let state = load_state(config);
    let new_state = sync_once(config, user_id, username, state, influx_ref).await?;

    if let Err(e) = save_state(config, &new_state) {
        error!("Failed to save state: {}", e);
    }

    Ok(())
}

async fn polling_loop(config: Config, user_id: String, username: String) {
    debug!("Entering polling_loop");
    
    // Get influx settings once
    let influx_enabled = config.influxdb_enabled();
    let influx_url = config.influx_url.clone();
    let influx_org = config.influx_org.clone();
    let influx_bucket = config.influx_bucket.clone();
    let influx_token = config.influx_token.clone();
    
    debug!("InfluxDB configured: {}", influx_enabled);
    debug!("poll_interval_seconds = {}", config.poll_interval_seconds);
    
    if config.poll_interval_seconds <= 0 {
        info!("Running single sync and exiting");
        if let Err(e) = run_sync(&config, &user_id, &username, None).await {
            error!("Sync failed: {}", e);
            std::process::exit(1);
        }
        debug!("Single sync completed, exiting with code 0");
        return;
    }

    loop {
        // Create influx client for this iteration if configured
        let influx_ref = if influx_enabled {
            let client = InfluxClient::new(
                influx_url.as_ref().unwrap(),
                influx_org.as_ref().unwrap(),
                influx_token.as_ref().unwrap()
            );
            Some((client, influx_bucket.as_ref().unwrap().as_str()))
        } else {
            None
        };
        
        match run_sync(&config, &user_id, &username, influx_ref).await {
            Ok(_) => {}
            Err(e) => {
                error!("Error during sync: {}", e);
            }
        }

        info!(
            "Sleeping {}s until next sync",
            config.poll_interval_seconds
        );
        sleep(Duration::from_secs(config.poll_interval_seconds as u64)).await;
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() {
    // Set panic handler for better error reporting
    panic::set_hook(Box::new(panic_handler));
    
    // Force stderr output for Docker logs
    debug!("Binary started at {}", Utc::now());
    debug!("Attempting to load .env file...");
    
    match dotenv::dotenv() {
        Ok(_) => debug!("Loaded .env file successfully"),
        Err(e) => debug!("No .env file found or error: {}", e),
    }

    debug!("Reading environment variables...");
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FATAL: Missing required config: {}", e);
            eprintln!("Required: CONCEPT2_API_TOKEN, CONCEPT2_INFLUX_* vars (optional)");
            std::process::exit(1);
        }
    };
    
    debug!("Config loaded successfully");
    env_logger::Builder::from_default_env()
        .filter_level(
            config
                .log_level
                .to_uppercase()
                .parse()
                .unwrap_or(log::LevelFilter::Info),
        )
        .format_module_path(false)
        .try_init()
        .ok();

    info!("Configuration:");
    info!("  concept2_api_base: {}", config.concept2_api_base);
    info!("  poll_interval_seconds: {}", config.poll_interval_seconds);
    info!("  state_file: {}", config.state_file.display());
    info!("  log_level: {}", config.log_level);
    info!("  influx_url: {}", config.influx_url.as_deref().unwrap_or("not set"));
    info!("  influx_org: {}", config.influx_org.as_deref().unwrap_or("not set"));
    info!("  influx_bucket: {}", config.influx_bucket.as_deref().unwrap_or("not set"));
    info!("  influx_token: {}", if config.influx_token.is_some() { "***" } else { "not set" });

    debug!("Authenticating with Concept2 API...");
    let (user_id, username) = match fetch_user_id(&config).await {
        Ok((user_id, username)) => (user_id, username),
        Err(e) => {
            error!("FATAL: Failed to authenticate: {}", e);
            std::process::exit(1);
        }
    };
    info!("Authenticated as user_id={} username={}", user_id, username);

    debug!("Starting polling loop...");
    polling_loop(config, user_id, username).await;
}
