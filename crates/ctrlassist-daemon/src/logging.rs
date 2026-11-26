//! Logging for the daemon: the journal when it runs as a service, the
//! terminal when a person runs it.

use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt};

/// Initialize tracing, with the `log` records of dependencies that don't
/// use tracing carried over to it.
///
/// `identifier` is the journal's `SYSLOG_IDENTIFIER`: the daemon shares
/// the `ctrlassist` binary with the clients, so the process name can't
/// tell its entries apart, and `journalctl -t` filters by this.
///
/// `directives` are filter strings such as `"ctrlassist_daemon=info"`.
/// `RUST_LOG` overrides them entirely when set.
///
/// Where the entries go depends on whether stderr is a terminal:
/// - not a terminal, with the journal reachable: the journal alone, with
///   structured fields and no second timestamp;
/// - not a terminal, no journal: stderr without timestamps;
/// - a terminal: stderr with timestamps, and the journal too if there.
///
/// # Install order (load-bearing)
///
/// `LogTracer` is installed *before* `set_global_default` because
/// `tracing-subscriber`'s `SubscriberInitExt::init()` / `set_default()`
/// auto-install a `LogTracer` of their own when the `tracing-log`
/// feature is enabled, and only one `LogTracer` can win globally. We use
/// `tracing::subscriber::set_global_default` instead, which doesn't
/// touch `LogTracer`, leaving ours intact.
///
/// # Errors
///
/// Fails if a directive doesn't parse or a logger is already installed.
pub fn init(identifier: &str, directives: &[&str]) -> anyhow::Result<()> {
    LogTracer::init()?;

    let env_filter = if std::env::var_os("RUST_LOG").is_some() {
        EnvFilter::from_default_env()
    } else {
        let mut filter = EnvFilter::from_default_env();
        for directive in directives {
            filter = filter.add_directive(directive.parse()?);
        }
        filter
    };

    let interactive = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let journald_layer = tracing_journald::layer()
        .ok()
        .map(|layer| layer.with_syslog_identifier(identifier.to_owned()));

    let subscriber = tracing_subscriber::registry().with(env_filter);
    match (interactive, journald_layer) {
        (true, journald) => {
            tracing::subscriber::set_global_default(subscriber.with(fmt::layer()).with(journald))?;
        }
        (false, Some(journald)) => {
            tracing::subscriber::set_global_default(subscriber.with(journald))?;
        }
        (false, None) => {
            tracing::subscriber::set_global_default(subscriber.with(fmt::layer().without_time()))?;
        }
    }
    Ok(())
}
