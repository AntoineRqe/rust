use once_cell::sync::OnceCell;
use tracing_subscriber::{EnvFilter};
use tracing_subscriber::fmt::format::FmtSpan;

#[cfg(not(test))]
use tracing_appender::non_blocking::WorkerGuard;

#[cfg(not(test))]
static TRACING: OnceCell<WorkerGuard> = OnceCell::new();

#[cfg(test)]
static TRACING: OnceCell<()> = OnceCell::new();

pub fn init_tracing(module: &str) {
    TRACING.get_or_init(|| {
        let file_appender =
            tracing_appender::rolling::never("./logs", format!("{}.log", module));

        #[cfg(test)]
        let writer = file_appender;

        #[cfg(not(test))]
        let (writer, guard) = tracing_appender::non_blocking(file_appender);

        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("debug"));

        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(writer)
            .with_span_events(FmtSpan::ENTER | FmtSpan::EXIT)
            .with_ansi(false)
            .with_target(false)
            .with_thread_ids(true)
            .with_thread_names(true)
            .init();

        #[cfg(test)]
        ();

        #[cfg(not(test))]
        guard
    });
}
