use env_logger;
use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};

struct FilterGilrsLogger(env_logger::Logger);

impl Log for FilterGilrsLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.0.enabled(metadata)
    }
    fn log(&self, record: &Record) {
        if record.level() == Level::Error
            && record
                .target()
                .starts_with("gilrs_core::platform::platform::gamepad")
            && record.args().to_string().contains("epoll failed: EINTR")
        {
            return;
        }
        self.0.log(record);
    }
    fn flush(&self) {
        self.0.flush();
    }
}

pub fn init_logger() -> Result<(), SetLoggerError> {
    let logger = env_logger::Builder::from_default_env().build();
    log::set_boxed_logger(Box::new(FilterGilrsLogger(logger)))?;
    log::set_max_level(LevelFilter::Info);
    Ok(())
}
