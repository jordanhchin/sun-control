//! Log viewer state: level parsing, filtering, search and scrolling.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Verbose,
    Debug,
    Info,
    Warning,
    Error,
    Fatal,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Verbose => "verbose",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "error",
            Level::Fatal => "fatal",
        }
    }

    const ALL: [Level; 6] = [
        Level::Verbose,
        Level::Debug,
        Level::Info,
        Level::Warning,
        Level::Error,
        Level::Fatal,
    ];

    /// Detect the level of a line in either the current (`[info]`) or the
    /// older (`: Info:`) Sunshine log format.
    pub fn parse(line: &str) -> Option<Level> {
        let head: String = line
            .chars()
            .take(96)
            .collect::<String>()
            .to_ascii_lowercase();
        Self::ALL.into_iter().find(|l| {
            let name = l.label();
            head.contains(&format!("[{name}]")) || head.contains(&format!(": {name}:"))
        })
    }
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub text: String,
    pub level: Level,
}

pub enum LogStatus {
    Loading,
    Loaded,
    Unsupported,
    Failed(String),
}

pub struct LogsState {
    pub lines: Vec<LogLine>,
    pub status: LogStatus,
    pub min_level: Level,
    pub follow: bool,
    /// Index into the filtered lines of the first visible row (when not following).
    pub top: usize,
    pub search: Option<String>,
    pub in_flight: bool,
    pub view_height: usize,
}

impl Default for LogsState {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            status: LogStatus::Loading,
            min_level: Level::Info,
            follow: true,
            top: 0,
            search: None,
            in_flight: false,
            view_height: 20,
        }
    }
}

/// Drop the thread id column (`[0x00007f…]`), which is noise when reading.
fn tidy(line: &str) -> String {
    if let Some(start) = line.find("] [0x")
        && let Some(len) = line[start + 2..].find("] ")
    {
        return format!("{}{}", &line[..start + 1], &line[start + 2 + len + 1..]);
    }
    line.to_owned()
}

impl LogsState {
    pub fn set_text(&mut self, text: &str) {
        let mut level = Level::Info;
        self.lines = text
            .lines()
            .map(|l| {
                // Continuation lines inherit the level of the entry above them.
                level = Level::parse(l).unwrap_or(level);
                LogLine {
                    text: tidy(l),
                    level,
                }
            })
            .collect();
        self.status = LogStatus::Loaded;
    }

    pub fn visible(&self) -> Vec<&LogLine> {
        self.lines
            .iter()
            .filter(|l| l.level >= self.min_level)
            .collect()
    }

    pub fn first_row(&self, total: usize) -> usize {
        let max_top = total.saturating_sub(self.view_height);
        if self.follow {
            max_top
        } else {
            self.top.min(max_top)
        }
    }

    pub fn scroll(&mut self, delta: isize) {
        let total = self.visible().len();
        let current = self.first_row(total) as isize;
        let max_top = total.saturating_sub(self.view_height) as isize;
        let next = (current + delta).clamp(0, max_top);
        self.top = next as usize;
        self.follow = next == max_top;
    }

    pub fn cycle_level(&mut self) {
        let i = Level::ALL
            .iter()
            .position(|l| *l == self.min_level)
            .unwrap_or(0);
        self.min_level = Level::ALL[(i + 1) % Level::ALL.len()];
    }

    /// Jump to the next (or previous) line containing the search term.
    pub fn find(&mut self, forward: bool) -> bool {
        let Some(term) = self.search.as_ref().map(|s| s.to_lowercase()) else {
            return false;
        };
        let lines = self.visible();
        let start = self.first_row(lines.len());
        let hit = if forward {
            (start + 1..lines.len()).find(|&i| lines[i].text.to_lowercase().contains(&term))
        } else {
            (0..start)
                .rev()
                .find(|&i| lines[i].text.to_lowercase().contains(&term))
        };
        drop(lines);
        match hit {
            Some(i) => {
                self.follow = false;
                self.top = i;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_levels_in_both_formats() {
        assert_eq!(
            Level::parse("[2026-10-21 09:55:55.1] [0x7f] [warning] [misc] x"),
            Some(Level::Warning)
        );
        assert_eq!(
            Level::parse("[2024:01:01:10:00:00]: Error: boom"),
            Some(Level::Error)
        );
        assert_eq!(Level::parse("   continuation"), None);
    }

    #[test]
    fn strips_thread_id() {
        assert_eq!(
            tidy("[2026-10-21 09:55] [0x00007fdd17ba9000] [info]    hi"),
            "[2026-10-21 09:55] [info]    hi"
        );
        assert_eq!(tidy("plain"), "plain");
    }

    #[test]
    fn continuation_lines_inherit_level() {
        let mut s = LogsState::default();
        s.set_text("[t] [error] bad\n  detail\n[t] [info] ok");
        assert_eq!(s.lines[1].level, Level::Error);
        s.min_level = Level::Error;
        assert_eq!(s.visible().len(), 2);
    }
}
