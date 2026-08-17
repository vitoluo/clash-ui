use chrono::{DateTime, FixedOffset, Utc};
use std::fmt::Arguments;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const LOG_DIRECTORY: &str = "data/logs";
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
        let date = current_date();
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

fn shanghai_now() -> DateTime<FixedOffset> {
    let offset =
        FixedOffset::east_opt(SHANGHAI_OFFSET_SECONDS).expect("Asia/Shanghai 时区偏移量必须有效");
    Utc::now().with_timezone(&offset)
}

fn current_date() -> String {
    shanghai_now().format("%Y%m%d").to_string()
}

fn write_console(message: &str) {
    std::eprintln!("{message}");
}

/// 初始化应用日志器；debug 构建输出控制台，release 构建写入按日分割的文件。
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
        let date = current_date();
        assert_eq!(date.len(), 8);
        assert!(date.chars().all(|character| character.is_ascii_digit()));
    }
}
