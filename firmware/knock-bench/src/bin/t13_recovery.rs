//! T13: soak with unplug / replug recovery.
//!
//! Wiring: the full array. Acquisition runs as in T4 (all FIFOs drained every
//! 20 ms) for 30 minutes. **During the run, unplug one sensor's cable for a
//! few seconds and plug it back in; repeat with other sensors (and, if you
//! like, the multiplexer's own SDA/SCL). Have every sensor plugged in again
//! before the end.**
//!
//! What the firmware does:
//!
//! - A sensor whose readout fails is logged ("I3 stopped answering"), taken
//!   out of the round-robin and probed every 500 ms (CHIP_ID through its
//!   channel, with a short 20 ms timeout). The other sensors keep being read.
//! - When it answers again it is re-initialised (soft reset + configuration)
//!   and put back; the outage duration is logged.
//! - A transfer that times out means the bus is stuck: the bus is dropped,
//!   `port.recover()` sends 9 SCL pulses + STOP, a new bus is built and the
//!   acquisition continues.
//! - The independent watchdog (IWDG, 2 s) is fed once per round. If the
//!   firmware ever hangs, the MCU resets, the test starts again and its banner
//!   is followed by "reset by watchdog", which fails the test.
//!
//! Reported: outages and recoveries per sensor, longest outage, re-init
//! failures, bus recoveries, and the longest gap between two successful
//! readouts of any sensor while it was healthy (20 ms nominal).
//!
//! Pass: not reset by the watchdog; every sensor that answered again was
//! re-initialised successfully; all sensors healthy at the end.
//!
//! Limits (the multiplexer's RESET pin is not wired):
//!
//! - Recovery pulses only reach the trunk and the multiplexer channel that is
//!   selected at that moment. That is the channel of the sensor being read
//!   when the bus hung (see the mux policy in `knock_bench::imu`), so the
//!   sensor that caused the hang is the one that gets the pulses. A sensor on
//!   another channel cannot hold the trunk, as it is disconnected from it.
//! - If a channel sensor keeps SDA low even after the pulses, the trunk stays
//!   blocked, because the multiplexer cannot be told to drop the channel over
//!   a blocked bus. Every sensor is then reported down until the line is
//!   released (unplug that sensor) or the array is power-cycled.
//! - If the multiplexer itself locks up, only a power cycle helps.
//! - The watchdog keeps running when a debugger halts the core; a breakpoint
//!   longer than 2 s resets the MCU.
//!
//! Build with `--features knock-bench/quick` for a 10 s smoke run. Ends with
//! the RESULT line, then LED2 blinks slowly on PASS and fast on FAIL.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Ticker, Timer};
use knock_bench::board::{self, Board};
use knock_bench::bus::{self, TimeoutI2c};
use knock_bench::drivers::health::{ErrorCounters, GapTracker, OutageTracker};
use knock_bench::imu::{self, Fault, ROUND_PERIOD, SENSORS};
use knock_bench::report::{self, Report};
use knock_bench::{check, fail, note, row};

/// Length of the run.
const DURATION: Duration = if cfg!(feature = "quick") {
    Duration::from_secs(10)
} else {
    Duration::from_secs(30 * 60)
};
/// Interval between summary lines.
const SUMMARY_EVERY: Duration = if cfg!(feature = "quick") {
    Duration::from_secs(5)
} else {
    Duration::from_secs(60)
};
/// Watchdog timeout. Must exceed the longest stretch without a round: a few
/// 200 ms transfer timeouts in a row plus a bus recovery.
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(2);
/// How often a sensor that is down is probed.
const PROBE_INTERVAL: Duration = Duration::from_millis(500);
/// Probe interval after a probe that hung the bus.
const PROBE_BACKOFF: Duration = Duration::from_secs(2);
/// Transfer timeout while probing a sensor that is down, so a dead branch
/// costs the healthy sensors as little time as possible.
const PROBE_TIMEOUT: Duration = Duration::from_millis(20);

/// What is tracked per sensor.
#[derive(Clone, Copy)]
struct SensorState {
    errors: ErrorCounters,
    outage: OutageTracker,
    gap: GapTracker,
    next_probe: Instant,
}

impl SensorState {
    const fn new() -> Self {
        Self {
            errors: ErrorCounters::new(),
            outage: OutageTracker::new(),
            gap: GapTracker::new(),
            next_probe: Instant::MIN,
        }
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let Board {
        i2c1: mut port,
        mut led2,
        ..
    } = Board::init();
    let watchdog_reset = imu::take_watchdog_reset_flag();

    let mut report = Report::begin("T13", "soak and recovery");
    if watchdog_reset {
        fail!(
            report,
            "reset by watchdog: the previous run hung for more than {} ms; the test has restarted",
            WATCHDOG_TIMEOUT.as_millis()
        );
    } else {
        note!("normal start (not a watchdog reset)");
    }
    note!(
        "I2C1 at {} Hz; {} s run; unplug and replug sensor cables now and then; watchdog {} ms",
        board::I2C1_HZ,
        DURATION.as_secs(),
        WATCHDOG_TIMEOUT.as_millis()
    );

    let mut array = imu::bring_up(&mut port).await;
    check!(
        report,
        array.ready_count() == SENSORS,
        "{} of {} sensors configured at the start",
        array.ready_count(),
        SENSORS
    );

    // From here on the firmware must feed the watchdog, also while idling.
    let mut watchdog = imu::start_watchdog(WATCHDOG_TIMEOUT);

    let started = Instant::now();
    let mut sensors = [SensorState::new(); SENSORS];
    // Sensors missing at the start are in an outage from the beginning, so
    // plugging one in later is handled like any other recovery.
    for (k, s) in sensors.iter_mut().enumerate() {
        if !array.is_ready(k) {
            s.outage.fail(started.as_micros());
        }
    }
    let mut reinit_failures = 0u32;
    let mut bus_recoveries = 0u32;
    let mut bus_still_stuck = 0u32;
    let mut next_summary = SUMMARY_EVERY;
    let mut probe_cursor = 0usize;

    'run: loop {
        {
            let mut i2c = TimeoutI2c::new(port.bus());
            let mut ticker = Ticker::every(ROUND_PERIOD);
            loop {
                ticker.next().await;
                watchdog.pet();
                let elapsed = started.elapsed();
                if elapsed >= DURATION {
                    array.park(&mut i2c).await;
                    break 'run;
                }

                // 1. Read every healthy sensor.
                let mut failed: [Option<Fault>; SENSORS] = [None; SENSORS];
                let round = array
                    .drain_round(&mut i2c, |k, result| match result {
                        Ok(drain) => {
                            sensors[k].errors.record_ok();
                            sensors[k].gap.data(drain.at.as_micros());
                        }
                        Err(fault) => {
                            sensors[k].errors.record(fault.class());
                            failed[k] = Some(fault);
                        }
                    })
                    .await;
                let now = Instant::now();
                for (k, fault) in failed.iter().enumerate() {
                    let Some(fault) = fault else { continue };
                    array.set_ready(k, false);
                    sensors[k].gap.reset();
                    sensors[k].next_probe = now + PROBE_INTERVAL;
                    if sensors[k].outage.fail(now.as_micros()) {
                        defmt::warn!(
                            "t = {=u64} s: I{=usize} stopped answering ({}); the others keep running",
                            elapsed.as_secs(),
                            k,
                            defmt::Debug2Format(fault)
                        );
                    }
                }
                let mut hung = round.aborted.is_some();

                // 2. Probe at most one sensor that is down.
                if !hung {
                    let due = (0..SENSORS)
                        .map(|i| (probe_cursor + i) % SENSORS)
                        .find(|&k| sensors[k].outage.is_down() && now >= sensors[k].next_probe);
                    if let Some(k) = due {
                        probe_cursor = (k + 1) % SENSORS;
                        sensors[k].next_probe = now + PROBE_INTERVAL;
                        i2c.set_timeout(PROBE_TIMEOUT);
                        let probed = array.probe(&mut i2c, k).await;
                        i2c.set_timeout(bus::DEFAULT_TIMEOUT);
                        match probed {
                            Ok(()) => match array.init_sensor(&mut i2c, k).await {
                                Ok(()) => {
                                    let back = Instant::now().as_micros();
                                    let outage_us = sensors[k].outage.recover(back).unwrap_or(0);
                                    note!(
                                        "t = {} s: I{} answers again and is re-initialised; outage lasted {} ms",
                                        elapsed.as_secs(),
                                        k,
                                        outage_us / 1000
                                    );
                                }
                                Err(fault) => {
                                    reinit_failures += 1;
                                    hung = fault.is_timeout();
                                    defmt::warn!(
                                        "t = {=u64} s: I{=usize} answers but re-initialisation failed: {}",
                                        elapsed.as_secs(),
                                        k,
                                        defmt::Debug2Format(&fault)
                                    );
                                }
                            },
                            // Still unplugged: stay quiet.
                            Err(fault) => {
                                if fault.is_timeout() {
                                    hung = true;
                                    sensors[k].next_probe = now + PROBE_BACKOFF;
                                }
                            }
                        }
                    }
                }

                if elapsed >= next_summary {
                    next_summary += SUMMARY_EVERY;
                    led2.toggle();
                    let down = sensors.iter().filter(|s| s.outage.is_down()).count();
                    note!(
                        "t = {} s: {} sensors down, {} outages, {} recovered, {} bus recoveries",
                        elapsed.as_secs(),
                        down,
                        sensors.iter().map(|s| s.outage.outages()).sum::<u32>(),
                        sensors.iter().map(|s| s.outage.recoveries()).sum::<u32>(),
                        bus_recoveries
                    );
                }
                if hung {
                    break;
                }
            }
        } // the bus is dropped here

        bus_recoveries += 1;
        let state = port.recover().await;
        if !state.is_idle() {
            bus_still_stuck += 1;
        }
        // Log the first few in full; a permanently stuck bus would flood the log.
        if bus_recoveries <= 20 {
            note!(
                "t = {} s: bus hung; recovery {}: {:?}",
                started.elapsed().as_secs(),
                bus_recoveries,
                state
            );
        }
        watchdog.pet();
    }
    led2.set_low();
    watchdog.pet();

    let end_us = Instant::now().as_micros();
    row!("sensor | outages | recovered | longest outage ms | longest gap ms | read errors | state");
    for (k, s) in sensors.iter().enumerate() {
        row!(
            "   I{}  | {:>7} | {:>9} | {:>17} | {:>14} | {:>11} | {}",
            k,
            s.outage.outages(),
            s.outage.recoveries(),
            s.outage.longest_outage_us() / 1000,
            s.gap.longest_us() / 1000,
            s.errors.errors(),
            match s.outage.open_outage_us(end_us) {
                Some(_) => "DOWN",
                None => "ok",
            }
        );
    }
    let outages: u32 = sensors.iter().map(|s| s.outage.outages()).sum();
    let recovered: u32 = sensors.iter().map(|s| s.outage.recoveries()).sum();
    let longest_gap_us = sensors
        .iter()
        .map(|s| s.gap.longest_us())
        .max()
        .unwrap_or(0);
    let longest_outage_us = sensors
        .iter()
        .map(|s| s.outage.longest_outage_us())
        .max()
        .unwrap_or(0);
    note!(
        "{} outages, {} recovered, longest finished outage {} ms",
        outages,
        recovered,
        longest_outage_us / 1000
    );
    note!(
        "longest gap in a healthy sensor's data: {} ms (nominal {} ms)",
        longest_gap_us / 1000,
        ROUND_PERIOD.as_millis()
    );
    note!(
        "{} bus recoveries, {} of them left the bus stuck",
        bus_recoveries,
        bus_still_stuck
    );
    if outages == 0 {
        note!("no outage was seen: unplug and replug a sensor during the run to exercise recovery");
    }

    check!(
        report,
        reinit_failures == 0,
        "{} re-initialisations failed after a sensor answered again",
        reinit_failures
    );
    for (k, s) in sensors.iter().enumerate() {
        if let Some(open_us) = s.outage.open_outage_us(end_us) {
            fail!(
                report,
                "I{} still down at the end (for {} ms): not plugged back in, or recovery did not work",
                k,
                open_us / 1000
            );
        }
    }
    let verdict = report.finish();
    // Like `report::idle_blink`, but the watchdog cannot be stopped and must
    // still be fed.
    let half_period = if verdict.is_pass() {
        report::BLINK_PASS
    } else {
        report::BLINK_FAIL
    };
    loop {
        led2.toggle();
        watchdog.pet();
        Timer::after(half_period).await;
    }
}
