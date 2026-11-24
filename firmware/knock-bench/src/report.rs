//! Common shape for bench-test output.
//!
//! # Log report (defmt over RTT)
//!
//! Every test prints the same structure, so logs can be read (and grepped) the
//! same way:
//!
//! ```text
//! === T02 mux scan ===
//! PASS: mux answers at 0x70
//! FAIL: ch3: 0x68 missing
//! INFO: i2c1 at 400000 Hz
//! RESULT: FAIL (1 failed)
//! ```
//!
//! [`Report::begin`] prints the banner, [`Report::check`] / [`Report::pass`] /
//! [`Report::fail`] print one line per check and count failures,
//! [`Report::finish`] prints the `RESULT` line. The [`check!`](crate::check),
//! [`pass!`](crate::pass), [`fail!`](crate::fail), [`note!`](crate::note) and
//! [`row!`](crate::row) macros take `core::fmt` format strings:
//!
//! ```ignore
//! let mut report = Report::begin("T02", "mux scan");
//! check!(report, id == 0xD1, "ch{}: CHIP_ID {:#04x}", k, id);
//! note!("i2c1 at {} Hz", board::I2C1_HZ);
//! let verdict = report.finish();
//! report::idle_blink(&mut led, verdict).await
//! ```
//!
//! Check messages are formatted on the target with `core::fmt` (not defmt's
//! deferred formatting), because the `PASS:` / `FAIL:` prefix is chosen at run
//! time. Use `{:?}` for error values. Free-form detail lines can use
//! `defmt::info!` directly.
//!
//! # CSV stream (USART1 / virtual COM port)
//!
//! [`CsvWriter`] writes line-oriented data for the host tools:
//!
//! ```text
//! # t05,t_ms,sensor,x,y,z
//! 12,0,-3,17,2051
//! 14,1,5,-9,2047
//! # end
//! ```
//!
//! A stream starts with `# <test>,<columns...>` and ends with `# end`; lines
//! are terminated by `\n`. Lines starting with `#` are metadata.

use core::fmt::{self, Write as _};

use embassy_stm32::gpio::Output;
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use heapless::String;

/// Final outcome of a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum Verdict {
    /// Every check passed.
    Pass,
    /// At least one check failed; the payload is the number of failed checks.
    Fail(u16),
}

impl Verdict {
    /// True for [`Verdict::Pass`].
    pub const fn is_pass(self) -> bool {
        matches!(self, Verdict::Pass)
    }
}

/// Collects the checks of one test and prints the common report shape.
pub struct Report {
    checks: u16,
    failed: u16,
}

impl Report {
    /// Start a report: prints the banner `=== <id> <title> ===`.
    ///
    /// `id` is the test identifier (`"T02"`), `title` a short name.
    pub fn begin(id: &str, title: &str) -> Self {
        defmt::info!("=== {=str} {=str} ===", id, title);
        Self {
            checks: 0,
            failed: 0,
        }
    }

    /// Record one check and print `PASS: <what>` or `FAIL: <what>`.
    ///
    /// Returns `ok`, so it can be used in a condition.
    pub fn check(&mut self, ok: bool, what: fmt::Arguments<'_>) -> bool {
        self.checks = self.checks.saturating_add(1);
        if ok {
            defmt::info!("PASS: {}", defmt::Display2Format(&what));
        } else {
            self.failed = self.failed.saturating_add(1);
            defmt::error!("FAIL: {}", defmt::Display2Format(&what));
        }
        ok
    }

    /// Record a passed check.
    pub fn pass(&mut self, what: fmt::Arguments<'_>) {
        self.check(true, what);
    }

    /// Record a failed check.
    pub fn fail(&mut self, what: fmt::Arguments<'_>) {
        self.check(false, what);
    }

    /// Number of checks recorded so far.
    pub const fn checks(&self) -> u16 {
        self.checks
    }

    /// Number of failed checks so far.
    pub const fn failed(&self) -> u16 {
        self.failed
    }

    /// The verdict so far, without printing anything.
    pub const fn verdict(&self) -> Verdict {
        if self.failed == 0 {
            Verdict::Pass
        } else {
            Verdict::Fail(self.failed)
        }
    }

    /// End the report: prints `RESULT: PASS (0 failed)` or
    /// `RESULT: FAIL (n failed)` and returns the verdict.
    pub fn finish(self) -> Verdict {
        let verdict = self.verdict();
        match verdict {
            Verdict::Pass => defmt::info!("RESULT: PASS (0 failed)"),
            Verdict::Fail(n) => defmt::error!("RESULT: FAIL ({=u16} failed)", n),
        }
        verdict
    }
}

/// Print an informational line `INFO: <what>` (not a check).
pub fn note(what: fmt::Arguments<'_>) {
    defmt::info!("INFO: {}", defmt::Display2Format(&what));
}

/// Print a free-form line (for example one table row) formatted with
/// `core::fmt`, with no prefix.
pub fn row(what: fmt::Arguments<'_>) {
    defmt::info!("{}", defmt::Display2Format(&what));
}

/// Record a check on a [`Report`](crate::report::Report) with a `core::fmt`
/// message: `check!(report, cond, "fmt", args...)`. Evaluates to `cond`.
#[macro_export]
macro_rules! check {
    ($report:expr, $cond:expr, $($fmt:tt)+) => {
        $report.check($cond, ::core::format_args!($($fmt)+))
    };
}

/// Record a passed check: `pass!(report, "fmt", args...)`.
#[macro_export]
macro_rules! pass {
    ($report:expr, $($fmt:tt)+) => {
        $report.pass(::core::format_args!($($fmt)+))
    };
}

/// Record a failed check: `fail!(report, "fmt", args...)`.
#[macro_export]
macro_rules! fail {
    ($report:expr, $($fmt:tt)+) => {
        $report.fail(::core::format_args!($($fmt)+))
    };
}

/// Print an `INFO:` line: `note!("fmt", args...)`.
#[macro_export]
macro_rules! note {
    ($($fmt:tt)+) => {
        $crate::report::note(::core::format_args!($($fmt)+))
    };
}

/// Print a free-form line, e.g. a table row: `row!("fmt", args...)`.
#[macro_export]
macro_rules! row {
    ($($fmt:tt)+) => {
        $crate::report::row(::core::format_args!($($fmt)+))
    };
}

/// LED2 half period while idling after a passed test (1 Hz blink).
pub const BLINK_PASS: Duration = Duration::from_millis(500);
/// LED2 half period while idling after a failed test (5 Hz blink).
pub const BLINK_FAIL: Duration = Duration::from_millis(100);

/// Idle forever, blinking the LED slowly on PASS and fast on FAIL.
pub async fn idle_blink(led: &mut Output<'_>, verdict: Verdict) -> ! {
    let half_period = if verdict.is_pass() {
        BLINK_PASS
    } else {
        BLINK_FAIL
    };
    loop {
        led.toggle();
        Timer::after(half_period).await;
    }
}

/// Maximum length of one CSV line in bytes, including the terminator.
pub const CSV_LINE_CAP: usize = 192;

/// Error from [`CsvWriter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum CsvError<E> {
    /// The line did not fit in [`CSV_LINE_CAP`] bytes; nothing was sent.
    LineTooLong,
    /// The underlying writer failed.
    Io(E),
}

/// Line-oriented CSV writer over any async byte sink (normally the VCP's
/// `UartTx`, or `&mut` to it).
///
/// Each line is formatted into a fixed buffer first and sent with one
/// `write_all`, so a line is never interleaved with a partial one.
pub struct CsvWriter<W> {
    out: W,
    line: String<CSV_LINE_CAP>,
}

impl<W: Write> CsvWriter<W> {
    /// Wrap a writer. Nothing is sent until [`CsvWriter::header`].
    pub const fn new(out: W) -> Self {
        Self {
            out,
            line: String::new(),
        }
    }

    /// Start a stream: sends `# <test>,<col1>,<col2>,...`.
    pub async fn header(&mut self, test: &str, columns: &[&str]) -> Result<(), CsvError<W::Error>> {
        self.line.clear();
        self.push_str("# ")?;
        self.push_str(test)?;
        for column in columns {
            self.push_str(",")?;
            self.push_str(column)?;
        }
        self.send().await
    }

    /// Send one data row from a format string:
    /// `csv.row(format_args!("{},{},{}", t, x, y)).await`.
    pub async fn row(&mut self, args: fmt::Arguments<'_>) -> Result<(), CsvError<W::Error>> {
        self.line.clear();
        self.line
            .write_fmt(args)
            .map_err(|_| CsvError::LineTooLong)?;
        self.send().await
    }

    /// Send one data row of comma-separated values of a single type.
    pub async fn values<T: fmt::Display>(
        &mut self,
        values: &[T],
    ) -> Result<(), CsvError<W::Error>> {
        self.line.clear();
        for (i, value) in values.iter().enumerate() {
            if i != 0 {
                self.push_str(",")?;
            }
            write!(self.line, "{value}").map_err(|_| CsvError::LineTooLong)?;
        }
        self.send().await
    }

    /// Send a metadata line `# <text>` in the middle of a stream.
    pub async fn comment(&mut self, text: fmt::Arguments<'_>) -> Result<(), CsvError<W::Error>> {
        self.line.clear();
        self.push_str("# ")?;
        self.line
            .write_fmt(text)
            .map_err(|_| CsvError::LineTooLong)?;
        self.send().await
    }

    /// End the stream: sends `# end` and flushes the writer.
    pub async fn end(&mut self) -> Result<(), CsvError<W::Error>> {
        self.line.clear();
        self.push_str("# end")?;
        self.send().await?;
        self.out.flush().await.map_err(CsvError::Io)
    }

    /// Give the writer back.
    pub fn into_inner(self) -> W {
        self.out
    }

    fn push_str<E>(&mut self, s: &str) -> Result<(), CsvError<E>> {
        self.line.push_str(s).map_err(|_| CsvError::LineTooLong)
    }

    /// Terminate the buffered line and send it.
    async fn send(&mut self) -> Result<(), CsvError<W::Error>> {
        self.line.push('\n').map_err(|_| CsvError::LineTooLong)?;
        self.out
            .write_all(self.line.as_bytes())
            .await
            .map_err(CsvError::Io)
    }
}
