//! T10 / T11: piezo arrival times, and (feature `long-window`) ring-down.
//!
//! Wiring: as for T9 (four piezo channels on Arduino A0..A3). Piezo positions
//! on the plate in cm, origin at the centre: P0 (−5, 3.33), P1 (5, 3.33),
//! P2 (5, −3.33), P3 (−5, −3.33); channel k = Pk. Run
//! `host/bench/capture.py` on the virtual COM port to record the CSV.
//!
//! LED2 on = armed. Knock on the plate: first on each piezo, then at the
//! centre. Each trigger (any channel more than 60 counts from its baseline)
//! captures −1 ms … +10 ms at the full rate and reports, per channel, the
//! onset time (first crossing of 8 σ of that channel's pre-trigger noise, at
//! least 6 counts, linearly interpolated and corrected for the 2.5 µs scan
//! skew per channel), the arrival order, the time differences to the earliest
//! channel in µs, and the peak amplitudes. The window is sent as CSV followed
//! by `# knock <n> onsets_us,<t0>,<t1>,<t2>,<t3>` (µs relative to the
//! trigger, `nan` = no onset found). LED2 is off and knocks are ignored during
//! the dump (about 0.4 s). Hold the blue button about half a second to finish.
//!
//! Expected (T10): the piezo you knock on arrives first; a centre knock gives
//! nearly equal arrival times. The firmware cannot know where you knocked, so
//! its own pass criteria are only: at least one knock captured, every knock
//! gave an onset on all four channels, and no samples were lost while armed.
//!
//! With `--features knock-bench/long-window` (T11) each trigger captures
//! −5 ms … +500 ms instead, decimated by 25 with peak hold (one row per
//! 250 µs holding the sample furthest from the baseline), and reports the
//! ring-down per channel: the time from the trigger until the signal stays
//! within the trigger threshold. The CSV has the same columns and is followed
//! by `# knock <n> ringdown_us,<r0>,<r1>,<r2>,<r3>` (`nan` = channel never
//! exceeded the threshold, `inf` = still ringing at the end of the window).
//! The dump takes about 0.8 s. Pass: at least one knock, every channel that
//! rang also settled within the window, no samples lost. The largest
//! ring-down seen is the lower bound for the refractory period.
//!
//! With `--features knock-bench/quick` the test ends by itself after 10 s.

#![no_std]
#![no_main]

use core::fmt;

use embassy_executor::Spawner;
use embassy_stm32::exti::ExtiInput;
use embassy_stm32::gpio::Output;
use embassy_stm32::mode::Async;
use embassy_stm32::usart::UartTx;
use embassy_time::{Duration, Instant, Timer};
use knock_bench::board::{Board, PiezoResources};
use knock_bench::drivers::piezo::{self as sig, CHANNELS, Recorder, Trigger};
#[cfg(not(feature = "long-window"))]
use knock_bench::drivers::stats;
use knock_bench::piezo::{self, ButtonPoll, Event, Piezo, PiezoError};
use knock_bench::report::{self, CsvWriter, Report};
use knock_bench::{check, fail, note, row};

/// The CSV sink: the virtual COM port transmitter.
type Csv = CsvWriter<UartTx<'static, Async>>;

/// CSV stream name.
const STREAM: &str = "t10_piezo_time";

/// The test ends by itself after this long (`None`: only the button ends it).
#[cfg(not(feature = "quick"))]
const TIME_LIMIT: Option<Duration> = None;
#[cfg(feature = "quick")]
const TIME_LIMIT: Option<Duration> = Some(Duration::from_secs(10));

/// Trigger threshold: deviation from the channel's baseline in counts
/// (60 counts = 48 mV). Also the ring-down threshold.
const TRIGGER_COUNTS: u16 = 60;

/// Timing window: full rate, 1 ms before the trigger and 10 ms from it on.
#[cfg(not(feature = "long-window"))]
mod window {
    pub const DECIMATION: usize = 1;
    pub const PRE: usize = 100;
    pub const POST: usize = 1000;
}
/// Ring-down window: 250 µs per entry (peak hold over 25 frames), 5 ms before
/// the trigger and 500 ms from it on.
#[cfg(feature = "long-window")]
mod window {
    pub const DECIMATION: usize = 25;
    pub const PRE: usize = 20;
    pub const POST: usize = 2000;
}
use window::{DECIMATION, POST, PRE};
const ENTRIES: usize = PRE + POST;
/// Time per window entry in microseconds.
const ENTRY_US: u32 = piezo::SAMPLE_PERIOD_US * DECIMATION as u32;

/// Onset threshold: `ONSET_K` standard deviations of the pre-trigger samples,
/// but at least `ONSET_MIN_COUNTS`.
#[cfg(not(feature = "long-window"))]
const ONSET_K: f32 = 8.0;
#[cfg(not(feature = "long-window"))]
const ONSET_MIN_COUNTS: f32 = 6.0;

/// Ring-down: the signal counts as settled once this many entries in a row
/// (80 × 250 µs = 20 ms) stay within the threshold.
#[cfg(feature = "long-window")]
const RING_QUIET_ENTRIES: usize = 80;

/// Give up after this many unexpected sample-loss events.
const MAX_OVERRUNS: u32 = 10;

/// A time in microseconds for logs and CSV: one decimal, or `nan`.
struct Us(Option<f32>);

impl fmt::Display for Us {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(t) if t.is_finite() => write!(f, "{t:.1}"),
            Some(t) if t.is_infinite() => f.write_str("inf"),
            _ => f.write_str("nan"),
        }
    }
}

/// Totals over the whole run.
#[derive(Default)]
struct Totals {
    knocks: u16,
    /// Knocks for which some channel gave no result (no onset / no settling).
    incomplete: u16,
    /// Largest ring-down seen, in microseconds (`long-window` only).
    #[cfg(feature = "long-window")]
    longest_ring_us: f32,
}

/// Analyse one window for arrival times. Returns the per-channel onset times
/// in microseconds relative to the trigger.
#[cfg(not(feature = "long-window"))]
fn analyse(
    knock: u16,
    trigger_channel: usize,
    _trigger: &Trigger,
    window: &sig::Window<'_>,
    scratch: &mut [i16],
    totals: &mut Totals,
) -> [Option<f32>; CHANNELS] {
    let mut onsets = [None; CHANNELS];
    let mut peaks = [0.0f32; CHANNELS];
    for ch in 0..CHANNELS {
        let n = window.copy_channel(ch, scratch);
        let samples = &scratch[..n];
        let pre = window.pre().min(n);
        let baseline = stats::mean(&samples[..pre]).unwrap_or(f32::from(sig::MID_SCALE));
        peaks[ch] = stats::peak_abs_dev(samples, baseline).unwrap_or(0.0);
        onsets[ch] = sig::onset(samples, pre, ONSET_K, ONSET_MIN_COUNTS).map(|o| {
            sig::onset_time_us(
                o.position,
                window.pre(),
                ENTRY_US as f32,
                ch,
                piezo::SCAN_SKEW_US,
            )
        });
    }

    let (order, found) = sig::arrival_order(&onsets);
    let earliest = (found != 0).then(|| onsets[order[0]]).flatten();
    let diff = |ch: usize| Us(onsets[ch].zip(earliest).map(|(t, t0)| t - t0));
    defmt::info!(
        "--- knock {=u16} (trigger on A{=usize}) ---",
        knock,
        trigger_channel
    );
    match found {
        4 => row!(
            "knock {} order: A{} A{} A{} A{} (earliest first)",
            knock,
            order[0],
            order[1],
            order[2],
            order[3]
        ),
        0 => row!("knock {} order: no onset on any channel", knock),
        _ => row!(
            "knock {} order: only {} of 4 channels have an onset, first A{}",
            knock,
            found,
            order[0]
        ),
    }
    row!(
        "knock {} dt to earliest [us]: A0 {} A1 {} A2 {} A3 {}",
        knock,
        diff(0),
        diff(1),
        diff(2),
        diff(3)
    );
    row!(
        "knock {} onset re trigger [us]: A0 {} A1 {} A2 {} A3 {}",
        knock,
        Us(onsets[0]),
        Us(onsets[1]),
        Us(onsets[2]),
        Us(onsets[3])
    );
    row!(
        "knock {} peak [counts]: A0 {:.0} A1 {:.0} A2 {:.0} A3 {:.0}",
        knock,
        peaks[0],
        peaks[1],
        peaks[2],
        peaks[3]
    );
    if found != CHANNELS {
        totals.incomplete = totals.incomplete.saturating_add(1);
    }
    onsets
}

/// Analyse one window for ring-down. Returns the per-channel ring-down time
/// in microseconds from the trigger (`None`: never above the threshold,
/// infinite: still ringing at the end of the window).
#[cfg(feature = "long-window")]
fn analyse(
    knock: u16,
    trigger_channel: usize,
    trigger: &Trigger,
    window: &sig::Window<'_>,
    scratch: &mut [i16],
    totals: &mut Totals,
) -> [Option<f32>; CHANNELS] {
    // The entries are peak-held, so the pre-trigger part is biased away from
    // the mean; use the trigger's tracked baseline instead.
    let baseline = trigger.baseline();
    let mut times = [None; CHANNELS];
    let mut settled = true;
    for ch in 0..CHANNELS {
        let n = window.copy_channel(ch, scratch);
        let outcome = sig::ring_down(
            &scratch[..n],
            f32::from(baseline[ch]),
            f32::from(TRIGGER_COUNTS),
            window.pre(),
            RING_QUIET_ENTRIES,
        );
        times[ch] = match outcome {
            sig::RingDown::Quiet => None,
            sig::RingDown::Settled { last_above } => {
                // The entry after the last one above the threshold.
                let t = ((last_above + 1 - window.pre()) as u32 * ENTRY_US) as f32;
                totals.longest_ring_us = totals.longest_ring_us.max(t);
                Some(t)
            }
            sig::RingDown::StillRinging { .. } => {
                settled = false;
                Some(f32::INFINITY)
            }
        };
    }
    defmt::info!(
        "--- knock {=u16} (trigger on A{=usize}) ---",
        knock,
        trigger_channel
    );
    row!(
        "knock {} ring-down to +-{} counts [us]: A0 {} A1 {} A2 {} A3 {} (nan = never above, inf = still ringing after {} ms)",
        knock,
        TRIGGER_COUNTS,
        Us(times[0]),
        Us(times[1]),
        Us(times[2]),
        Us(times[3]),
        POST as u32 * ENTRY_US / 1000
    );
    if !settled {
        totals.incomplete = totals.incomplete.saturating_add(1);
    }
    times
}

/// Name of the per-knock result line that follows the CSV rows.
#[cfg(not(feature = "long-window"))]
const RESULT_KEY: &str = "onsets_us";
#[cfg(feature = "long-window")]
const RESULT_KEY: &str = "ringdown_us";

/// Send one knock: the window rows, then the result line.
async fn send_knock(
    csv: &mut Csv,
    knock: u16,
    window: &sig::Window<'_>,
    results: &[Option<f32>; CHANNELS],
) -> Result<(), report::CsvError<embassy_stm32::usart::Error>> {
    piezo::write_window(csv, STREAM, knock, window, DECIMATION).await?;
    csv.comment(format_args!(
        "knock {} {},{},{},{},{}",
        knock,
        RESULT_KEY,
        Us(results[0]),
        Us(results[1]),
        Us(results[2]),
        Us(results[3])
    ))
    .await
}

async fn run(
    report: &mut Report,
    resources: PiezoResources,
    button: &ExtiInput<'static, Async>,
    led: &mut Output<'static>,
    csv: &mut Csv,
) {
    let mut adc = match Piezo::new(resources) {
        Ok(adc) => adc,
        Err(e) => {
            fail!(report, "ADC start failed: {:?}", e);
            return;
        }
    };
    let (Some(storage), Some(scratch)) = (
        cortex_m::singleton!(: [u16; ENTRIES * CHANNELS] = [0; ENTRIES * CHANNELS]),
        cortex_m::singleton!(: [i16; ENTRIES] = [0; ENTRIES]),
    ) else {
        fail!(report, "capture buffers already taken");
        return;
    };
    let Some(mut recorder) = Recorder::new(&mut storage[..], PRE, POST, DECIMATION) else {
        fail!(report, "capture buffer too small");
        return;
    };
    note!(
        "ADC1 A0..A3: {} S/s per channel, scan skew {} ns per channel; window {} + {} entries of {} us; trigger {} counts",
        piezo::SAMPLE_RATE_HZ,
        piezo::SCAN_SKEW_NS,
        PRE,
        POST,
        ENTRY_US,
        TRIGGER_COUNTS
    );
    // Let the front end and the ADC settle after reset.
    Timer::after(Duration::from_millis(200)).await;

    let mut trigger = Trigger::new(TRIGGER_COUNTS);
    let mut button = ButtonPoll::new(button);
    let deadline = TIME_LIMIT.map(|d| Instant::now() + d);
    let mut totals = Totals::default();
    let mut csv_failed = false;
    let mut lost = 0u32;

    note!(
        "knock on each piezo, then at the centre (LED2 on = armed); hold the button 0.5 s to finish"
    );
    loop {
        let event = match adc.settle(&mut trigger).await {
            Ok(()) => {
                led.set_high();
                let event = adc
                    .capture(&mut trigger, &mut recorder, &mut button, deadline)
                    .await;
                led.set_low();
                event
            }
            Err(e) => Err(e),
        };
        match event {
            Ok(Event::Knock { channel }) => {
                totals.knocks = totals.knocks.saturating_add(1);
                let Some(window) = recorder.window() else {
                    continue;
                };
                let results = analyse(
                    totals.knocks,
                    channel,
                    &trigger,
                    &window,
                    &mut scratch[..],
                    &mut totals,
                );
                if !csv_failed
                    && let Err(e) = send_knock(csv, totals.knocks, &window, &results).await
                {
                    csv_failed = true;
                    defmt::warn!("CSV write failed ({}); no more CSV output", e);
                }
            }
            Ok(Event::Button) => {
                note!("button pressed: finishing");
                break;
            }
            Ok(Event::Timeout) => {
                note!("quick mode: time limit reached");
                break;
            }
            Err(PiezoError::Overrun) if lost < MAX_OVERRUNS => {
                lost += 1;
                defmt::warn!("samples lost while armed (event {=u32}); re-arming", lost);
            }
            Err(PiezoError::NotQuiet) => {
                fail!(
                    report,
                    "inputs never settled within {} counts for 50 ms: floating or noisy input, or threshold too low",
                    TRIGGER_COUNTS
                );
                break;
            }
            Err(e) => {
                fail!(report, "acquisition stopped: {:?}", e);
                break;
            }
        }
    }
    if let Err(e) = csv.end().await {
        defmt::warn!("CSV end marker not sent: {}", e);
    }

    check!(
        report,
        totals.knocks > 0,
        "{} knock(s) captured",
        totals.knocks
    );
    #[cfg(not(feature = "long-window"))]
    check!(
        report,
        totals.incomplete == 0,
        "{} knock(s) without an onset on all four channels (a channel too weak or not connected)",
        totals.incomplete
    );
    #[cfg(feature = "long-window")]
    {
        check!(
            report,
            totals.incomplete == 0,
            "{} knock(s) still ringing at the end of the {} ms window",
            totals.incomplete,
            POST as u32 * ENTRY_US / 1000
        );
        note!(
            "longest ring-down: {:.1} ms; the refractory period must be longer",
            totals.longest_ring_us / 1000.0
        );
    }
    check!(
        report,
        adc.overruns() == 0,
        "samples lost while armed: {} event(s)",
        adc.overruns()
    );
    check!(report, !csv_failed, "CSV output on the virtual COM port");
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let Board {
        vcp,
        mut led2,
        button,
        piezo: resources,
        ..
    } = Board::init();

    #[cfg(not(feature = "long-window"))]
    let mut report = Report::begin("T10", "piezo time");
    #[cfg(feature = "long-window")]
    let mut report = Report::begin("T11", "piezo ring-down");
    let (tx, _rx) = vcp.split();
    let mut csv = CsvWriter::new(tx);

    run(&mut report, resources, &button, &mut led2, &mut csv).await;

    let verdict = report.finish();
    report::idle_blink(&mut led2, verdict).await
}
