//! T9: piezo front end: idle level, noise, and what a hard knock does.
//!
//! Wiring: the four piezo channels on Arduino A0..A3 through the front end of
//! proposal §3.3 (shared VMID ≈ 1.65 V, 1 MΩ bleed, 10 kΩ series, two 1N4148
//! clamps, 1 nF to ground). Measure VMID with the multimeter separately.
//! Optional: `host/bench/capture.py` on the virtual COM port for the CSV.
//!
//! Phase 1 (5 s, do not touch the plate): mean and RMS noise per channel.
//! Pass: every mean within 2048 ± 150 counts, every RMS below 8 counts, and
//! the delivered sample rate within 1 % of 100 kS/s per channel.
//!
//! Phase 2 (until the blue button is pressed): LED2 on = armed. Knock on or
//! near each piezo, including the hardest knock you intend to use. For every
//! trigger (any channel more than 60 counts from its baseline) the log shows,
//! per channel, min / max / peak deviation, whether the waveform flat-topped
//! (5 or more consecutive samples within 3 counts of the extreme: a clamp or
//! a rail was reached) and a warning if a sample sat on an ADC rail (0 or
//! 4095). The window from −1 ms to +10 ms goes out as CSV at the full rate
//! (decimation 1, about 0.4 s per knock at 921600 baud; LED2 is off and
//! knocks are ignored meanwhile). Hold the button for about half a second to
//! finish.
//!
//! Final pass criteria: all four channels responded to at least one knock
//! (peak deviation above the trigger threshold), no samples were lost while
//! armed, and the firmware got here. Ends with the RESULT line; LED2 then
//! blinks slowly on PASS, fast on FAIL.
//!
//! With `--features knock-bench/quick`: phase 1 lasts 1 s and phase 2 ends by
//! itself after 10 s.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_stm32::exti::ExtiInput;
use embassy_stm32::gpio::Output;
use embassy_stm32::mode::Async;
use embassy_stm32::usart::UartTx;
use embassy_time::{Duration, Instant, Timer};
use knock_bench::board::{Board, PiezoResources};
use knock_bench::drivers::piezo::{self as sig, CHANNELS, Recorder, Trigger};
use knock_bench::drivers::stats;
use knock_bench::piezo::{self, ButtonPoll, Event, Piezo, PiezoError};
use knock_bench::report::{self, CsvWriter, Report};
use knock_bench::{check, fail, note, row};

/// The CSV sink: the virtual COM port transmitter.
type Csv = CsvWriter<UartTx<'static, Async>>;

/// CSV stream name.
const STREAM: &str = "t09_piezo_dc";

/// Length of the idle measurement in frames (100 000 frames = 1 s).
#[cfg(not(feature = "quick"))]
const IDLE_FRAMES: u32 = 500_000;
#[cfg(feature = "quick")]
const IDLE_FRAMES: u32 = 100_000;

/// Phase 2 ends by itself after this long (`None`: only the button ends it).
#[cfg(not(feature = "quick"))]
const KNOCK_PHASE_LIMIT: Option<Duration> = None;
#[cfg(feature = "quick")]
const KNOCK_PHASE_LIMIT: Option<Duration> = Some(Duration::from_secs(10));

/// Expected idle level and its tolerance, in counts.
const IDLE_TARGET: f32 = 2048.0;
const IDLE_TOLERANCE: f32 = 150.0;
/// Idle RMS noise limit in counts (8 counts = 6.4 mV).
const IDLE_RMS_LIMIT: f32 = 8.0;
/// Accepted relative error of the delivered sample rate.
const RATE_TOLERANCE: f32 = 0.01;

/// Trigger threshold: deviation from the channel's baseline in counts
/// (60 counts = 48 mV).
const TRIGGER_COUNTS: u16 = 60;
/// Window: 100 frames (1 ms) before the trigger, 1000 (10 ms) from it on.
const PRE_FRAMES: usize = 100;
const POST_FRAMES: usize = 1000;
const WINDOW_FRAMES: usize = PRE_FRAMES + POST_FRAMES;
/// CSV decimation: none. 1100 rows of about 34 bytes take about 0.4 s.
const DECIMATION: usize = 1;

/// Flat-top detection: this many consecutive samples within `FLAT_TOLERANCE`
/// counts of the extreme, the extreme at least `FLAT_MIN_DEV` from baseline.
const FLAT_RUN: usize = 5;
const FLAT_TOLERANCE: i16 = 3;
const FLAT_MIN_DEV: f32 = 200.0;

/// Give up after this many unexpected sample-loss events.
const MAX_OVERRUNS: u32 = 10;

/// Phase 1: idle statistics. Returns the per-channel baselines.
async fn idle_phase(report: &mut Report, adc: &mut Piezo) -> Option<sig::Frame> {
    note!(
        "phase 1: idle for {} ms, do not touch the plate",
        IDLE_FRAMES / (piezo::SAMPLE_RATE_HZ / 1000)
    );
    let run = match adc.idle(IDLE_FRAMES).await {
        Ok(run) => run,
        Err(e) => {
            fail!(report, "idle measurement aborted: {:?}", e);
            return None;
        }
    };

    let nominal = piezo::SAMPLE_RATE_HZ as f32;
    match run.rate_hz {
        Some(rate) => {
            check!(
                report,
                (rate - nominal).abs() <= RATE_TOLERANCE * nominal,
                "sample rate {:.0} S/s per channel (expected {} within 1 %)",
                rate,
                piezo::SAMPLE_RATE_HZ
            );
        }
        None => fail!(report, "sample rate could not be measured"),
    }

    for (ch, acc) in run.stats.ch.iter().enumerate() {
        let mean = acc.mean().unwrap_or(0.0);
        let rms = acc.std_dev().unwrap_or(0.0);
        note!(
            "A{}: mean {:.1} counts ({} mV), RMS {:.2}, min {}, max {} over {} samples",
            ch,
            mean,
            sig::counts_to_mv((mean + 0.5) as u16, piezo::VDDA_MV),
            rms,
            acc.min().unwrap_or(0),
            acc.max().unwrap_or(0),
            acc.count()
        );
        check!(
            report,
            (mean - IDLE_TARGET).abs() <= IDLE_TOLERANCE,
            "A{}: idle mean {:.1} counts (expected {:.0} +- {:.0}; far off: check VMID, the 1 M bleed and this channel's wiring)",
            ch,
            mean,
            IDLE_TARGET,
            IDLE_TOLERANCE
        );
        check!(
            report,
            rms < IDLE_RMS_LIMIT,
            "A{}: idle RMS {:.2} counts (limit {:.0}; high: floating input, missing 1 nF, or hum on VMID)",
            ch,
            rms,
            IDLE_RMS_LIMIT
        );
    }
    Some(run.stats.means())
}

/// Print the per-channel summary of one captured knock and mark the channels
/// that responded.
fn describe_knock(
    knock: u16,
    trigger_channel: usize,
    window: &sig::Window<'_>,
    scratch: &mut [i16],
    responded: &mut [bool; CHANNELS],
) {
    defmt::info!(
        "--- knock {=u16} (trigger on A{=usize}) ---",
        knock,
        trigger_channel
    );
    for (ch, seen) in responded.iter_mut().enumerate() {
        let n = window.copy_channel(ch, scratch);
        let samples = &scratch[..n];
        let baseline = stats::mean(&samples[..window.pre().min(n)]).unwrap_or(IDLE_TARGET);
        let Some(ext) = sig::extremes(samples, FLAT_TOLERANCE) else {
            continue;
        };
        let peak = ext.peak_dev(baseline);
        if peak > f32::from(TRIGGER_COUNTS) {
            *seen = true;
        }
        let high = ext.flat_top_high(baseline, FLAT_MIN_DEV, FLAT_RUN);
        let low = ext.flat_top_low(baseline, FLAT_MIN_DEV, FLAT_RUN);
        row!(
            "knock {} A{}: min {} max {} peak dev {:.0} counts ({} mV){}{}{}",
            knock,
            ch,
            ext.min,
            ext.max,
            peak,
            sig::counts_to_mv(peak as u16, piezo::VDDA_MV),
            if high { ", FLAT-TOP high" } else { "" },
            if low { ", FLAT-TOP low" } else { "" },
            if ext.at_rail() { ", AT ADC RAIL" } else { "" }
        );
        if ext.at_rail() {
            defmt::warn!(
                "knock {=u16} A{=usize}: a sample sat on an ADC rail (0 or 4095): the pin went beyond 0..3.3 V and the clamp path carried current; consider the 22 k / 470 pF option of proposal 3.3",
                knock,
                ch
            );
        }
    }
}

/// Phase 2: capture knocks until the button (or the quick-mode limit).
async fn knock_phase(
    report: &mut Report,
    adc: &mut Piezo,
    baseline: sig::Frame,
    button: &ExtiInput<'static, Async>,
    led: &mut Output<'static>,
    csv: &mut Csv,
) {
    let (Some(storage), Some(scratch)) = (
        cortex_m::singleton!(: [u16; WINDOW_FRAMES * CHANNELS] = [0; WINDOW_FRAMES * CHANNELS]),
        cortex_m::singleton!(: [i16; WINDOW_FRAMES] = [0; WINDOW_FRAMES]),
    ) else {
        fail!(report, "capture buffers already taken");
        return;
    };
    let Some(mut recorder) = Recorder::new(&mut storage[..], PRE_FRAMES, POST_FRAMES, DECIMATION)
    else {
        fail!(report, "capture buffer too small");
        return;
    };
    let mut trigger = Trigger::new(TRIGGER_COUNTS);
    trigger.set_baseline(baseline);
    let mut button = ButtonPoll::new(button);
    let deadline = KNOCK_PHASE_LIMIT.map(|d| Instant::now() + d);

    note!(
        "phase 2: knock on each piezo (LED2 on = armed, threshold {} counts); hold the button 0.5 s to finish",
        TRIGGER_COUNTS
    );
    let mut responded = [false; CHANNELS];
    let mut knocks = 0u16;
    let mut csv_failed = false;
    let mut lost = 0u32;
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
                knocks = knocks.saturating_add(1);
                let Some(window) = recorder.window() else {
                    continue;
                };
                describe_knock(knocks, channel, &window, &mut scratch[..], &mut responded);
                if !csv_failed
                    && let Err(e) =
                        piezo::write_window(csv, STREAM, knocks, &window, DECIMATION).await
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
                note!("quick mode: knock phase time limit reached");
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

    note!("{} knock(s) captured", knocks);
    for (ch, seen) in responded.iter().enumerate() {
        check!(
            report,
            *seen,
            "A{}: responded to at least one knock (peak deviation above {} counts)",
            ch,
            TRIGGER_COUNTS
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
    note!(
        "ADC1 A0..A3: {} S/s per channel, scan skew {} ns per channel, 12 bit, {} mV full scale assumed",
        piezo::SAMPLE_RATE_HZ,
        piezo::SCAN_SKEW_NS,
        piezo::VDDA_MV
    );
    // Let the front end and the ADC settle after reset.
    Timer::after(Duration::from_millis(200)).await;

    let Some(baseline) = idle_phase(report, &mut adc).await else {
        return;
    };
    knock_phase(report, &mut adc, baseline, button, led, csv).await;
    pass_note(report);
}

/// The last pass criterion: the firmware is still running.
fn pass_note(report: &mut Report) {
    check!(
        report,
        true,
        "firmware still running at the end of the test"
    );
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

    let mut report = Report::begin("T09", "piezo dc");
    let (tx, _rx) = vcp.split();
    let mut csv = CsvWriter::new(tx);

    run(&mut report, resources, &button, &mut led2, &mut csv).await;

    let verdict = report.finish();
    report::idle_blink(&mut led2, verdict).await
}
