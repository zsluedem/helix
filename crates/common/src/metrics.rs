use axum::{
    body::Body,
    http::{header::CONTENT_TYPE, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use eyre::bail;
use lazy_static::lazy_static;
use prometheus::{
    register_histogram_vec_with_registry, register_histogram_with_registry,
    register_int_counter_vec_with_registry, register_int_counter_with_registry, Encoder, Histogram,
    HistogramTimer, HistogramVec, IntCounter, IntCounterVec, Registry, TextEncoder,
};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::{error, info, trace};

pub fn start_metrics_server() {
    let port =
        std::env::var("METRICS_PORT").map(|s| s.parse().expect("invalid port")).unwrap_or(9500);
    tokio::spawn(MetricsProvider::new(port).run());
}

pub struct MetricsProvider {
    port: u16,
}

impl MetricsProvider {
    pub fn new(port: u16) -> Self {
        MetricsProvider { port }
    }

    pub async fn run(self) -> eyre::Result<()> {
        info!("Starting metrics server on port {}", self.port);

        let router = axum::Router::new()
            .route("/metrics", get(handle_metrics))
            .route("/status", get(handle_status));
        let address = SocketAddr::from(([0, 0, 0, 0], self.port));
        let listener = TcpListener::bind(&address).await?;

        axum::serve(listener, router).await?;

        bail!("Metrics server stopped")
    }
}

async fn handle_status() -> Response {
    trace!("Handling status request");

    StatusCode::OK.into_response()
}

async fn handle_metrics() -> Response {
    trace!("Handling metrics request");

    match prepare_metrics() {
        Ok(response) => response,
        Err(err) => {
            error!(?err, "failed to prepare metrics");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn prepare_metrics() -> Result<Response, MetricsError> {
    let metrics = RELAY_METRICS_REGISTRY.gather();
    let encoder = TextEncoder::new();
    let s = encoder.encode_to_string(&metrics)?;

    Response::builder()
        .status(200)
        .header(CONTENT_TYPE, encoder.format_type())
        .body(Body::from(s))
        .map_err(MetricsError::FailedBody)
}

#[derive(Debug, thiserror::Error)]
enum MetricsError {
    #[error("failed encoding metrics {0}")]
    FailedEncoding(#[from] prometheus::Error),

    #[error("failed encoding body {0}")]
    FailedBody(#[from] axum::http::Error),
}

lazy_static! {
    static ref RELAY_METRICS_REGISTRY: Registry =
        Registry::new_custom(Some("helix".to_string()), None).unwrap();

    //////////////// API ////////////////

    /// Count for requests by API and endpoint
    static ref REQUEST_COUNTS: IntCounterVec = register_int_counter_vec_with_registry!(
        "request_count_total",
        "Count of requests",
        &["endpoint"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    /// Count for status codes by API and endpoint
    static ref REQUEST_STATUS: IntCounterVec =
        register_int_counter_vec_with_registry!(
        "request_status_total",
        "Count of status codes",
        &["endpoint", "http_status_code"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    /// Duration of request in seconds
    static ref REQUEST_LATENCY: HistogramVec = register_histogram_vec_with_registry!(
        "request_latency_sec",
        "Latency of requests",
        &["endpoint"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    /// Request size in bytes
    static ref REQUEST_SIZE: IntCounterVec = register_int_counter_vec_with_registry!(
        "request_size_bytes",
        "Size of requests",
        &["endpoint"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    //////////////// SIMULATOR ////////////////
    static ref SIMULATOR_COUNTS: IntCounterVec = register_int_counter_vec_with_registry!(
        "simulator_count_total",
        "Count of sim requests",
        &["is_optimistic"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    static ref SIMULATOR_STATUS: IntCounterVec = register_int_counter_vec_with_registry!(
        "simulator_status_total",
        "Count of sim statuses",
        &["is_success"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    static ref SIMULATOR_LATENCY: Histogram = register_histogram_with_registry!(
        "sim_latency_sec",
        "Latency of simulations",
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    static ref BUILDER_DEMOTION_COUNT: IntCounter = register_int_counter_with_registry!(
        "builder_demotion_count_total",
        "Count of builder demotions",
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    //////////////// GOSSIP ////////////////

    /// Received gossip messages coutn
     static ref IN_GOSSIP_COUNTS: IntCounterVec = register_int_counter_vec_with_registry!(
        "in_gossip_count_total",
        "Count of received gossip messages",
        &["endpoint"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();


    /// Received gossip size in bytes
    static ref IN_GOSSIP_SIZE: IntCounterVec = register_int_counter_vec_with_registry!(
        "in_gossip_size_bytes",
        "Size of receivedgossip messages",
        &["endpoint"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    /// Sent gossip messages count
    static ref OUT_GOSSIP_COUNTS: IntCounterVec = register_int_counter_vec_with_registry!(
        "out_gossip_count_total",
        "Count of sent gossip messages",
        &["endpoint", "is_success"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    /// Sent gossip latency
    static ref OUT_GOSSIP_LATENCY: HistogramVec = register_histogram_vec_with_registry!(
        "out_gossip_latency_sec",
        "Latency of sent gossip messages",
        &["endpoint"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    /// Sent gossip size in bytes
    static ref OUT_GOSSIP_SIZE: IntCounterVec = register_int_counter_vec_with_registry!(
        "out_gossip_size_bytes",
        "Size of sent gossip messages",
        &["endpoint"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    //////////////// DB ////////////////
    static ref DB_COUNTS: IntCounterVec = register_int_counter_vec_with_registry!(
        "db_count_total",
        "Count of db operations",
        &["endpoint", "is_success"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    static ref DB_LATENCY: HistogramVec = register_histogram_vec_with_registry!(
        "db_latency_sec",
        "Latency of db operations",
        &["endpoint"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();


    //////////////// REDIS ////////////////
    static ref REDIS_COUNTS: IntCounterVec = register_int_counter_vec_with_registry!(
        "redis_count_total",
        "Count of redis operations",
        &["endpoint", "is_success"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();

    static ref REDIS_LATENCY: HistogramVec = register_histogram_vec_with_registry!(
        "redis_latency_sec",
        "Latency of redis operations",
        &["endpoint"],
        &RELAY_METRICS_REGISTRY
    )
    .unwrap();
}

pub struct ApiMetrics;

impl ApiMetrics {
    pub fn count(endpoint: &str) {
        REQUEST_COUNTS.with_label_values(&[endpoint]).inc();
    }
    pub fn status(endpoint: &str, status_code: &str) {
        REQUEST_STATUS.with_label_values(&[endpoint, status_code]).inc();
    }
    /// Records on drop
    pub fn timer(endpoint: &str) -> HistogramTimer {
        REQUEST_LATENCY.with_label_values(&[endpoint]).start_timer()
    }
    pub fn size(endpoint: &str, size: usize) {
        REQUEST_SIZE.with_label_values(&[endpoint]).inc_by(size as u64);
    }
}

pub struct GossipMetrics;

impl GossipMetrics {
    pub fn in_count(endpoint: &str) {
        IN_GOSSIP_COUNTS.with_label_values(&[endpoint]).inc();
    }

    pub fn in_size(endpoint: &str, size: usize) {
        IN_GOSSIP_SIZE.with_label_values(&[endpoint]).inc_by(size as u64);
    }

    pub fn out_count(endpoint: &str, is_success: bool) {
        OUT_GOSSIP_COUNTS.with_label_values(&[endpoint, is_success.to_string().as_str()]).inc();
    }

    /// Records on drop
    pub fn out_timer(endpoint: &str) -> HistogramTimer {
        OUT_GOSSIP_LATENCY.with_label_values(&[endpoint]).start_timer()
    }

    pub fn out_size(endpoint: &str, size: usize) {
        OUT_GOSSIP_SIZE.with_label_values(&[endpoint]).inc_by(size as u64);
    }
}

pub struct DbMetrics;

impl DbMetrics {
    pub fn count(endpoint: &str, is_success: bool) {
        DB_COUNTS.with_label_values(&[endpoint, is_success.to_string().as_str()]).inc();
    }

    pub fn latency(endpoint: &str) -> HistogramTimer {
        DB_LATENCY.with_label_values(&[endpoint]).start_timer()
    }
}

pub struct DbMetricRecord<'a> {
    endpoint: &'a str,
    has_recorded: bool,
    _timer: HistogramTimer,
}

impl<'a> DbMetricRecord<'a> {
    pub fn new(endpoint: &'a str) -> Self {
        let timer = DbMetrics::latency(endpoint);
        DbMetricRecord { has_recorded: false, _timer: timer, endpoint }
    }

    pub fn record_success(&mut self) {
        self.has_recorded = true;
        DbMetrics::count(self.endpoint, true);
    }

    pub fn record_failure(&mut self) {
        self.has_recorded = true;
        DbMetrics::count(self.endpoint, false);
    }
}

impl<'a> Drop for DbMetricRecord<'a> {
    fn drop(&mut self) {
        if !self.has_recorded {
            self.record_failure();
        }
    }
}

pub struct RedisMetrics;

impl RedisMetrics {
    pub fn count(endpoint: &str, is_success: bool) {
        REDIS_COUNTS.with_label_values(&[endpoint, is_success.to_string().as_str()]).inc();
    }

    pub fn latency(endpoint: &str) -> HistogramTimer {
        REDIS_LATENCY.with_label_values(&[endpoint]).start_timer()
    }
}

pub struct RedisMetricRecord<'a> {
    endpoint: &'a str,
    has_recorded: bool,
    _timer: HistogramTimer,
}

impl<'a> RedisMetricRecord<'a> {
    pub fn new(endpoint: &'a str) -> Self {
        let timer = RedisMetrics::latency(endpoint);
        RedisMetricRecord { has_recorded: false, _timer: timer, endpoint }
    }

    pub fn record_success(&mut self) {
        self.has_recorded = true;
        RedisMetrics::count(self.endpoint, true);
    }

    pub fn record_failure(&mut self) {
        self.has_recorded = true;
        RedisMetrics::count(self.endpoint, false);
    }
}

impl<'a> Drop for RedisMetricRecord<'a> {
    fn drop(&mut self) {
        if !self.has_recorded {
            self.record_failure();
        }
    }
}

pub struct SimulatorMetrics;

impl SimulatorMetrics {
    pub fn sim_count(is_optimistic: bool) {
        SIMULATOR_COUNTS.with_label_values(&[is_optimistic.to_string().as_str()]).inc();
    }

    pub fn sim_status(is_success: bool) {
        SIMULATOR_STATUS.with_label_values(&[is_success.to_string().as_str()]).inc();
    }

    pub fn timer() -> HistogramTimer {
        SIMULATOR_LATENCY.start_timer()
    }

    pub fn demotion_count() {
        BUILDER_DEMOTION_COUNT.inc();
    }
}
