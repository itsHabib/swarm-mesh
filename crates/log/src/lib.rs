use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, fmt};

/// Initializes the logging subsystem for the application.
///
/// This function sets up structured logging using the tracing ecosystem,
/// configured for JSON output with detailed span information. The logging
/// level can be controlled via the RUST_LOG environment variable.
///
/// # Usage
/// Set RUST_LOG environment variable to control logging:
/// * `RUST_LOG=debug` - Verbose debugging information
/// * `RUST_LOG=info` - General information messages
/// * `RUST_LOG=warn` - Warning messages only
/// * `RUST_LOG=error` - Error messages only
pub fn init_logging() {
    LogTracer::init().expect("Failed to set logger");

    let subscriber = fmt::Subscriber::builder()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .with_current_span(true)
        .with_span_events(fmt::format::FmtSpan::CLOSE) // optional
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set global subscriber");
}
