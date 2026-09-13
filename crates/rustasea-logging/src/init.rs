//! Subscriber installation — compose Laravel channels into a `tracing` stack.
//!
//! [`init`] builds a subscriber from a [`LoggingConfig`] and installs it as the
//! process-global default exactly once. The returned [`LoggingGuard`] owns the
//! non-blocking file-appender workers; it must be kept alive for the lifetime
//! of the process or the log writers shut down (dropping it flushes and stops
//! the workers).
//!
//! Installation is idempotent: a second [`init`] call — or any pre-existing
//! global subscriber — is treated as a no-op that returns an empty guard rather
//! than panicking. This uses `try_init` internally.

use std::collections::BTreeMap;

use tracing::Subscriber;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, Registry};

use rustasea_config::ConfigLoader;

use crate::config::{
    ChannelConfig, Driver, LoggingConfig, DEFAULT_DAILY_FILES, DEFAULT_LOG_PATH,
    DEFAULT_MONTHLY_FILES,
};
use crate::error::{LoggingError, Result};

/// A boxed, thread-safe subscriber ready to be installed.
pub type BoxedSubscriber = Box<dyn Subscriber + Send + Sync + 'static>;

/// Owns the logging workers installed by [`init`].
///
/// Dropping the guard flushes and shuts down every non-blocking appender
/// worker. Hold it for as long as logging is required (typically the whole
/// `main`). An empty guard (from an idempotent second init) owns no workers.
#[must_use = "dropping the guard shuts down the logging workers"]
pub struct LoggingGuard {
    /// Non-blocking appender workers; dropping them flushes pending lines.
    workers: Vec<WorkerGuard>,
    /// Whether this guard actually installed a subscriber.
    installed: bool,
}

impl LoggingGuard {
    /// True when this call installed the global subscriber (false for the
    /// idempotent no-op case).
    pub fn is_installed(&self) -> bool {
        self.installed
    }

    /// Number of non-blocking appender workers owned by this guard.
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }
}

impl std::fmt::Debug for LoggingGuard {
    /// Renders the guard without exposing worker internals.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoggingGuard")
            .field("installed", &self.installed)
            .field("workers", &self.workers.len())
            .finish()
    }
}

/// Build a subscriber from `config` without installing it.
///
/// Returns the boxed subscriber together with the [`LoggingGuard`] that owns
/// the file-appender workers. This is the seam used by [`init`] and by tests
/// that prefer a thread-local subscriber ([`tracing::subscriber::with_default`])
/// over the process-global one.
///
/// # Errors
///
/// [`LoggingError::UnsupportedDriver`] for `slack`/`papertrail`/`syslog`,
/// [`LoggingError::UnknownChannel`] for a missing channel or stack member,
/// [`LoggingError::CyclicStack`] for a self-referential stack, and
/// [`LoggingError::InvalidConfig`] for an unparseable level or empty stack.
pub fn build(config: &LoggingConfig) -> Result<(BoxedSubscriber, LoggingGuard)> {
    let mut builder = LayerBuilder::new(&config.channels);
    let root = config.default_channel()?;
    builder.expand(&config.default, root)?;

    let subscriber: BoxedSubscriber = Box::new(Registry::default().with(builder.layers));
    Ok((
        subscriber,
        LoggingGuard {
            workers: builder.workers,
            installed: true,
        },
    ))
}

/// Install a global subscriber from `config` (idempotent).
///
/// The first successful call installs the subscriber and returns a guard that
/// owns the appender workers. Subsequent calls — or any other component having
/// already set a global subscriber — return an empty, non-installed guard
/// instead of panicking.
///
/// # Errors
///
/// Propagates the errors of [`build`]. An already-installed subscriber is
/// *not* an error.
pub fn init(config: &LoggingConfig) -> Result<LoggingGuard> {
    let (subscriber, guard) = build(config)?;
    match subscriber.try_init() {
        Ok(()) => Ok(guard),
        // A global subscriber already exists: idempotent no-op. The workers
        // built above are dropped, which is safe because the active subscriber
        // owns its own workers.
        Err(_) => Ok(LoggingGuard {
            workers: Vec::new(),
            installed: false,
        }),
    }
}

/// Load `[logging]` from `loader` and install the subscriber (idempotent).
///
/// Convenience wrapper around [`LoggingConfig::from_loader`] + [`init`].
///
/// # Errors
///
/// Propagates configuration and [`build`] errors.
pub fn init_from_config(loader: &ConfigLoader) -> Result<LoggingGuard> {
    let config = LoggingConfig::from_loader(loader)?;
    init(&config)
}

/// Accumulates the composed layers and the appender workers they require.
///
/// Borrows the channel map from the [`LoggingConfig`] for the duration of
/// expansion, so stack members resolve without cloning the whole config.
struct LayerBuilder<'a> {
    /// All defined channels, keyed by name.
    channels: &'a BTreeMap<String, ChannelConfig>,
    /// One boxed layer per concrete channel leaf.
    layers: Vec<Box<dyn Layer<Registry> + Send + Sync>>,
    /// Non-blocking appender workers that must outlive the subscriber.
    workers: Vec<WorkerGuard>,
    /// Chain of stack names currently being expanded (cycle detection).
    visiting: Vec<String>,
}

impl<'a> LayerBuilder<'a> {
    /// Create a builder over the given channel map.
    fn new(channels: &'a BTreeMap<String, ChannelConfig>) -> Self {
        Self {
            channels,
            layers: Vec::new(),
            workers: Vec::new(),
            visiting: Vec::new(),
        }
    }

    /// Expand `channel` into concrete layers, recursing through stacks.
    ///
    /// `name` is the channel's own key; the builder tracks the chain of stacks
    /// being expanded so a direct or transitive cycle is rejected with
    /// [`LoggingError::CyclicStack`].
    fn expand(&mut self, name: &str, channel: &ChannelConfig) -> Result<()> {
        let driver = channel.resolve_driver()?;
        if !driver.is_supported() {
            return Err(LoggingError::UnsupportedDriver(driver.name().to_string()));
        }
        match driver {
            Driver::Stack => self.expand_stack(name, channel),
            Driver::Null => Ok(()),
            _ => self.push_leaf(driver, channel),
        }
    }

    /// Expand a `stack` channel into its members' layers.
    fn expand_stack(&mut self, name: &str, channel: &ChannelConfig) -> Result<()> {
        if channel.channels.is_empty() {
            return Err(LoggingError::InvalidConfig(format!(
                "stack `{name}` declares no channels"
            )));
        }
        if self.visiting.iter().any(|seen| seen == name) {
            return Err(LoggingError::CyclicStack(name.to_string()));
        }
        self.visiting.push(name.to_string());
        let result = (|| {
            for member in &channel.channels {
                let child = self
                    .channels
                    .get(member)
                    .ok_or_else(|| LoggingError::UnknownChannel(member.to_string()))?;
                self.expand(member, child)?;
            }
            Ok(())
        })();
        self.visiting.pop();
        result
    }

    /// Push the concrete layer(s) for a non-stack driver.
    fn push_leaf(&mut self, driver: Driver, channel: &ChannelConfig) -> Result<()> {
        let level = parse_level(channel.level.as_deref().unwrap_or("debug"))?;
        match driver {
            Driver::Stderr | Driver::Errorlog => {
                self.layers.push(stderr_layer(level));
                Ok(())
            }
            Driver::Stdout => {
                self.layers.push(stdout_layer(level));
                Ok(())
            }
            Driver::Single | Driver::Emergency => {
                let path = channel.path.as_deref().unwrap_or(DEFAULT_LOG_PATH);
                let (writer, guard) = file_writer(path, Rotation::Never, None)?;
                self.workers.push(guard);
                self.layers.push(file_layer(writer, level));
                Ok(())
            }
            Driver::Daily => {
                let path = channel.path.as_deref().unwrap_or(DEFAULT_LOG_PATH);
                let max = channel.max_files.or(Some(DEFAULT_DAILY_FILES));
                let (writer, guard) = file_writer(path, Rotation::Daily, max)?;
                self.workers.push(guard);
                self.layers.push(file_layer(writer, level));
                Ok(())
            }
            Driver::Monthly => {
                // `tracing-appender` has no monthly rotation; the closest
                // supported interval is daily. See the crate-level docs.
                let path = channel.path.as_deref().unwrap_or(DEFAULT_LOG_PATH);
                let max = channel.max_files.or(Some(DEFAULT_MONTHLY_FILES));
                let (writer, guard) = file_writer(path, Rotation::Daily, max)?;
                self.workers.push(guard);
                self.layers.push(file_layer(writer, level));
                Ok(())
            }
            Driver::Stack => unreachable!("stack handled by expand_stack"),
            Driver::Null => Ok(()),
            Driver::Slack | Driver::Papertrail | Driver::Syslog => {
                Err(LoggingError::UnsupportedDriver(driver.name().to_string()))
            }
        }
    }
}

/// File rotation choice used by the file drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rotation {
    /// No rotation: a fixed file name.
    Never,
    /// One file per day.
    Daily,
}

/// Build a non-blocking writer for `path` plus the guard that owns its worker.
///
/// `path` is split into a directory and a file name. `Daily` rotation inserts a
/// date between the file stem and its extension (`rustasea-2026-01-01.log`),
/// matching Laravel's daily naming.
fn file_writer(
    path: &str,
    rotation: Rotation,
    max_files: Option<usize>,
) -> Result<(NonBlocking, WorkerGuard)> {
    let path = std::path::Path::new(path);
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| LoggingError::Io(format!("invalid log path: {}", path.display())))?;
    let (stem, suffix) = split_name(file_name);

    let appender = match rotation {
        Rotation::Never => tracing_appender::rolling::never(directory, file_name),
        Rotation::Daily => {
            let mut builder = tracing_appender::rolling::RollingFileAppender::builder()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix(format!("{stem}-"));
            if let Some(suffix) = suffix {
                builder = builder.filename_suffix(suffix);
            }
            if let Some(max) = max_files {
                builder = builder.max_log_files(max);
            }
            builder
                .build(directory)
                .map_err(|error| LoggingError::Io(error.to_string()))?
        }
    };
    Ok(tracing_appender::non_blocking(appender))
}

/// Split `name` into its stem and optional extension.
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, suffix)) if !stem.is_empty() => (stem, Some(suffix)),
        _ => (name, None),
    }
}

/// Parse a level string into a [`LevelFilter`].
///
/// # Errors
///
/// [`LoggingError::InvalidConfig`] when `level` is not a known level.
fn parse_level(level: &str) -> Result<LevelFilter> {
    level
        .parse::<LevelFilter>()
        .map_err(|_| LoggingError::InvalidConfig(format!("invalid log level `{level}`")))
}

/// A formatted layer writing to standard error.
fn stderr_layer(level: LevelFilter) -> Box<dyn Layer<Registry> + Send + Sync> {
    fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_filter(level)
        .boxed()
}

/// A formatted layer writing to standard output.
fn stdout_layer(level: LevelFilter) -> Box<dyn Layer<Registry> + Send + Sync> {
    fmt::layer()
        .with_writer(std::io::stdout)
        .with_target(true)
        .with_filter(level)
        .boxed()
}

/// A formatted layer writing to a non-blocking file appender.
fn file_layer(writer: NonBlocking, level: LevelFilter) -> Box<dyn Layer<Registry> + Send + Sync> {
    fmt::layer()
        .with_writer(writer)
        .with_target(true)
        .with_ansi(false)
        .with_filter(level)
        .boxed()
}
