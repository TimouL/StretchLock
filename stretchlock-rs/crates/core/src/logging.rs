use std::fs;
use std::path::Path;

use log::LevelFilter;
use simplelog::{CombinedLogger, Config, SharedLogger, SimpleLogger, WriteLogger};

pub const LOG_FILENAME: &str = "stretchlock.log";

pub fn init(level: &str, dir: Option<&Path>) {
    let filter = parse_level(level);
    if filter == LevelFilter::Off {
        log::set_max_level(LevelFilter::Off);
        return;
    }
    let mut loggers: Vec<Box<dyn SharedLogger>> = Vec::new();
    let mut file_attached = false;
    if let Some(dir) = dir {
        if fs::create_dir_all(dir).is_ok() {
            if let Some(file) = open_log_file(dir) {
                loggers.push(WriteLogger::new(LevelFilter::Trace, Config::default(), file));
                file_attached = true;
            }
        }
    }
    loggers.push(SimpleLogger::new(filter, Config::default()));
    let _ = CombinedLogger::init(loggers);
    log::set_max_level(if file_attached {
        LevelFilter::Trace
    } else {
        filter
    });
}

pub fn set_level(level: &str) {
    log::set_max_level(parse_level(level));
}

pub fn parse_level(level: &str) -> LevelFilter {
    match level.to_uppercase().as_str() {
        "DEBUG" => LevelFilter::Debug,
        "INFO" => LevelFilter::Info,
        "WARNING" => LevelFilter::Warn,
        "ERROR" => LevelFilter::Error,
        "OFF" => LevelFilter::Off,
        _ => LevelFilter::Info,
    }
}

fn open_log_file(dir: &Path) -> Option<fs::File> {
    fs::File::options()
        .create(true)
        .append(true)
        .open(dir.join(LOG_FILENAME))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::io::Write;

    #[test]
    fn parse_level_maps_known_names() {
        assert_eq!(parse_level("DEBUG"), LevelFilter::Debug);
        assert_eq!(parse_level("info"), LevelFilter::Info);
        assert_eq!(parse_level("WARNING"), LevelFilter::Warn);
        assert_eq!(parse_level("ERROR"), LevelFilter::Error);
        assert_eq!(parse_level("OFF"), LevelFilter::Off);
        assert_eq!(parse_level("nonsense"), LevelFilter::Info);
        assert_eq!(parse_level(""), LevelFilter::Info);
    }

    #[test]
    fn init_off_writes_no_log_file() {
        let dir = temp_dir("log-off");
        init("OFF", Some(&dir));
        assert!(!dir.join(LOG_FILENAME).exists());
        assert_eq!(log::max_level(), LevelFilter::Off);
    }

    #[test]
    fn open_log_file_appends() {
        let dir = temp_dir("log-append");
        {
            let mut file = open_log_file(&dir).unwrap();
            file.write_all(b"one,").unwrap();
        }
        {
            let mut file = open_log_file(&dir).unwrap();
            file.write_all(b"two").unwrap();
        }
        let text = fs::read_to_string(dir.join(LOG_FILENAME)).unwrap();
        assert_eq!(text, "one,two");
    }
}
