use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use std::fmt::Arguments;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const LOG_DIRECTORY: &str = "data/logs";
const LOG_RETENTION_DAYS: i64 = 30;
const SHANGHAI_OFFSET_SECONDS: i32 = 8 * 60 * 60;

static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();

enum Logger {
    Console,
    File(FileLogger),
}

struct FileLogger {
    directory: PathBuf,
    date: String,
    file: File,
}

impl FileLogger {
    fn new(root: &Path) -> io::Result<Self> {
        let directory = root.join(LOG_DIRECTORY);
        let today = shanghai_now().date_naive();
        let date = today.format("%Y%m%d").to_string();
        fs::create_dir_all(&directory)?;
        if let Err(error) = cleanup_old_log_files(&directory, today) {
            write_console(&format!("清理过期日志失败: {error}"));
        }
        let file = open_log_file(&directory, &date)?;
        Ok(Self {
            directory,
            date,
            file,
        })
    }

    fn write(&mut self, message: &str) -> io::Result<()> {
        let now = shanghai_now();
        let date = now.format("%Y%m%d").to_string();
        if self.date != date {
            self.file = open_log_file(&self.directory, &date)?;
            self.date = date;
        }

        writeln!(
            self.file,
            "{} [ERROR] {message}",
            now.format("%Y-%m-%d %H:%M:%S%.3f")
        )?;
        self.file.flush()
    }
}

fn open_log_file(directory: &Path, date: &str) -> io::Result<File> {
    fs::create_dir_all(directory)?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(format!("{date}.log")))
}

fn cleanup_old_log_files(directory: &Path, today: NaiveDate) -> io::Result<()> {
    let cutoff = today - Duration::days(LOG_RETENTION_DAYS);
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(date) = parse_log_date(&file_name) else {
            continue;
        };
        if date < cutoff {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn parse_log_date(file_name: &str) -> Option<NaiveDate> {
    let date = file_name.strip_suffix(".log")?;
    if date.len() != 8 || !date.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    NaiveDate::parse_from_str(date, "%Y%m%d").ok()
}

fn shanghai_now() -> DateTime<FixedOffset> {
    let offset =
        FixedOffset::east_opt(SHANGHAI_OFFSET_SECONDS).expect("Asia/Shanghai 时区偏移量必须有效");
    Utc::now().with_timezone(&offset)
}

fn write_console(message: &str) {
    std::eprintln!("{message}");
}

/// 初始化应用日志器；debug 构建输出控制台，release 构建写入按日分割的文件并清理超过 30 天的日志。
pub fn init(root: &Path) {
    let logger = if cfg!(debug_assertions) {
        Logger::Console
    } else {
        match FileLogger::new(root) {
            Ok(logger) => Logger::File(logger),
            Err(error) => {
                write_console(&format!("初始化文件日志失败: {error}"));
                Logger::Console
            }
        }
    };

    let _ = LOGGER.set(Mutex::new(logger));
}

/// 写入一条错误级别日志。
pub fn error(args: Arguments<'_>) {
    let message = args.to_string();
    let Some(logger) = LOGGER.get() else {
        write_console(&message);
        return;
    };

    let Ok(mut logger) = logger.lock() else {
        write_console(&message);
        return;
    };

    let result = match &mut *logger {
        Logger::Console => {
            write_console(&message);
            Ok(())
        }
        Logger::File(logger) => logger.write(&message),
    };
    if let Err(error) = result {
        write_console(&format!("写入文件日志失败: {error}; {message}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shanghai_date_uses_expected_file_name_format() {
        let date = shanghai_now().format("%Y%m%d").to_string();
        assert_eq!(date.len(), 8);
        assert!(date.chars().all(|character| character.is_ascii_digit()));
    }

    #[test]
    fn cleanup_removes_logs_older_than_30_days() {
        let directory = std::env::temp_dir().join(format!(
            "clash-ui-log-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&directory).unwrap();

        let today = NaiveDate::from_ymd_opt(2026, 8, 25).unwrap();
        let old_date = today - Duration::days(LOG_RETENTION_DAYS + 1);
        let retained_date = today - Duration::days(LOG_RETENTION_DAYS);
        fs::write(
            directory.join(format!("{}.log", old_date.format("%Y%m%d"))),
            "",
        )
        .unwrap();
        fs::write(
            directory.join(format!("{}.log", retained_date.format("%Y%m%d"))),
            "",
        )
        .unwrap();
        fs::write(directory.join("notes.log"), "").unwrap();

        cleanup_old_log_files(&directory, today).unwrap();

        assert!(!directory
            .join(format!("{}.log", old_date.format("%Y%m%d")))
            .exists());
        assert!(directory
            .join(format!("{}.log", retained_date.format("%Y%m%d")))
            .exists());
        assert!(directory.join("notes.log").exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
