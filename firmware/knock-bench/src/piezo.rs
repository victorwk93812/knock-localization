//! Piezo acquisition: ADC1 scanning Arduino A0..A3 into a circular DMA buffer.
//!
//! Channel order (= scan order): ch0 = A0 = PC5 = ADC1_IN14, ch1 = A1 = PC4 =
//! ADC1_IN13, ch2 = A2 = PC3 = ADC1_IN4, ch3 = A3 = PC2 = ADC1_IN3; 12-bit,
//! right-aligned, single-ended.
//!
//! # Sampling rate
//!
//! embassy-stm32 0.6.0 offers exactly one repeating mode for this ADC family
//! (`adc_v3`): free-running continuous conversion of the scan sequence into a
//! circular DMA buffer (`Adc::into_ring_buffered` with the `CONTINUOUS`
//! trigger; a timer trigger is only implemented for other families). The rate
//! is therefore set by the ADC clock and the conversion length alone:
//!
//! ```text
//! ADC clock        = SYSCLK 80 MHz / prescaler 8              = 10 MHz
//! one conversion   = 12.5 (sampling) + 12.5 (12-bit SAR)      = 25 cycles = 2.5 µs
//! one scan         = 4 conversions                            = 100 cycles = 10 µs
//! per-channel rate = 10 MHz / 100                             = 100 000 S/s exactly
//! aggregate rate   = 10 MHz / 25                              = 400 000 S/s
//! scan skew        = one conversion between adjacent channels = 2.5 µs
//! ```
//!
//! Without the prescaler (80 MHz) the nearest choices would be 76.9 kS/s
//! (247.5-cycle sampling) or 190.5 kS/s (92.5), so the prescaler is set here
//! through the PAC: the HAL has no API for it on this family. The rate is
//! exact relative to SYSCLK, i.e. as accurate as HSI16 (±1 %). There is no
//! jitter and no trigger latency, but the scan phase is not locked to any
//! other timer; the bench tests check the delivered rate against the embassy
//! time base ([`IdleRun::rate_hz`]).
//!
//! UNVERIFIED: the `ADC_CCR.PRESC` field (bits 21:18, `0b0100` = divide by 8)
//! and the 12.5-cycle SAR time are taken from memory of RM0351; the reference
//! manual is not in `references/` and stm32-metapac 21 does not model the
//! field. The rate check in `t09_piezo_dc` fails if either is wrong.
//!
//! # Sampling time
//!
//! 12.5 ADC cycles at 10 MHz = 1.25 µs. The source is 10 kΩ with 1 nF from
//! the pin to ground. For a bare 10 kΩ source the datasheet would ask for
//! about 3 µs on these (slow) channels, but the 1 nF sits directly at the pin
//! and is 200 times the ADC's sample capacitor (about 5 pF): the sample
//! capacitor charges from the 1 nF through the internal switch (about 1 kΩ ×
//! 5 pF = 5 ns), settling to 12 bits in well under 0.1 µs. What remains is
//! charge sharing: each conversion moves the pin by at most 5 pF / 1 nF =
//! 0.5 % of the difference to the previously converted channel, and the 10 kΩ
//! refills the 1 nF with τ = 10 µs, one scan period. With all channels idling
//! at mid-rail that is far below one count; during a knock it is a crosstalk
//! term of well under 1 % of the difference between adjacent channels. A
//! longer sampling time would not reduce it (the 10 kΩ cannot recharge 1 nF
//! within any available sampling time) and would cost the 100 kS/s rate.
//!
//! UNVERIFIED: the ADC figures above (5 pF, 1 kΩ, required sampling time per
//! source resistance) are from memory of the STM32L475 datasheet, which is
//! not in `references/`.
//!
//! # Buffers and overruns
//!
//! The DMA ring holds [`RING_SAMPLES`] samples (20.48 ms); it is read in
//! chunks of half that ([`CHUNK_SAMPLES`], 10.24 ms). If the reader falls a
//! full ring behind, the next read reports [`PiezoError::Overrun`], the HAL
//! resynchronises to the DMA position (frame-aligned, so channels never
//! shift) and reading can continue. The tests leave the ADC running while
//! they dump a capture over the serial port and call [`Piezo::settle`]
//! afterwards, which discards the backlog first.
//!
//! # Interrupts
//!
//! [`PiezoIrqs`] binds `DMA1_CHANNEL1`. The `ADC1_2` vector is not needed:
//! the embassy driver for this ADC family polls and uses DMA, it has no
//! interrupt handler.

use embassy_stm32::adc::{Adc, AdcChannel, CONTINUOUS, RingBufferedAdc, SampleTime};
use embassy_stm32::exti::ExtiInput;
use embassy_stm32::mode::Async;
use embassy_stm32::{bind_interrupts, dma, pac, peripherals};
use embassy_time::{Duration, Instant, with_timeout};
use knock_drivers::piezo::{self, CHANNELS, Frame, IdleStats, Recorder, Trigger};

use crate::board::{PiezoResources, SYSCLK_HZ};
use crate::report::{CsvError, CsvWriter};

bind_interrupts!(
    /// Interrupt binding for the ADC's DMA channel.
    pub struct PiezoIrqs {
        DMA1_CHANNEL1 => dma::InterruptHandler<peripherals::DMA1_CH1>;
    }
);

/// ADC clock prescaler (kernel clock = SYSCLK).
pub const ADC_PRESCALER: u32 = 8;
/// `ADC_CCR.PRESC` encoding of [`ADC_PRESCALER`].
const CCR_PRESC_DIV8: u32 = 0b0100;
/// Position and mask of `ADC_CCR.PRESC`.
const CCR_PRESC_SHIFT: u32 = 18;
const CCR_PRESC_MASK: u32 = 0xF << CCR_PRESC_SHIFT;

/// ADC clock in Hz (10 MHz).
pub const ADC_CLOCK_HZ: u32 = SYSCLK_HZ / ADC_PRESCALER;
/// Sampling time of every piezo channel: 12.5 ADC cycles (see module docs).
pub const SAMPLE_TIME: SampleTime = SampleTime::CYCLES12_5;
/// Length of one conversion in half ADC cycles: 2 × (12.5 + 12.5).
const CONVERSION_HALF_CYCLES: u32 = 25 + 25;

/// Sample rate per channel in Hz.
pub const SAMPLE_RATE_HZ: u32 = 100_000;
/// Sample period per channel in microseconds.
pub const SAMPLE_PERIOD_US: u32 = 10;
/// Delay between adjacent channels of one scan, in nanoseconds: channel `k` is
/// sampled `k × 2.5 µs` after channel 0.
pub const SCAN_SKEW_NS: u32 = 2_500;
/// [`SCAN_SKEW_NS`] in microseconds.
pub const SCAN_SKEW_US: f32 = SCAN_SKEW_NS as f32 / 1000.0;

// The constants above must agree with the clock arithmetic.
const _: () = {
    let conversions_per_s = ADC_CLOCK_HZ * 2 / CONVERSION_HALF_CYCLES;
    assert!(conversions_per_s == SAMPLE_RATE_HZ * CHANNELS as u32);
    assert!(SAMPLE_PERIOD_US * SAMPLE_RATE_HZ == 1_000_000);
    assert!(SCAN_SKEW_NS * CHANNELS as u32 == SAMPLE_PERIOD_US * 1000);
};

/// Nominal analog supply in millivolts, used to convert counts to volts.
/// The real VDDA is not measured.
pub const VDDA_MV: u32 = 3300;

/// DMA ring size in samples (2048 frames = 20.48 ms).
pub const RING_SAMPLES: usize = 8192;
/// Samples per [`Piezo::read_chunk`] (1024 frames = 10.24 ms).
pub const CHUNK_SAMPLES: usize = RING_SAMPLES / 2;
/// Frames per chunk.
pub const CHUNK_FRAMES: usize = CHUNK_SAMPLES / CHANNELS;

/// A chunk normally arrives every 10.24 ms; this long without one means the
/// ADC or its DMA has stopped.
const READ_TIMEOUT: Duration = Duration::from_millis(200);
/// Quiet chunks in a row required by [`Piezo::settle`] (about 50 ms).
const SETTLE_QUIET_CHUNKS: u8 = 5;
/// [`Piezo::settle`] gives up after this long.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
/// Chunks the button must read low to count as pressed (about 30 ms).
const BUTTON_LOW_CHUNKS: u8 = 3;

/// What can go wrong while acquiring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum PiezoError {
    /// The reader fell more than one ring (20 ms) behind; samples were lost.
    /// The stream has been resynchronised and can be read again.
    Overrun,
    /// No data for 200 ms although the ADC reports no error.
    Stalled,
    /// The ADC flagged a hardware overrun (a conversion finished before DMA
    /// had fetched the previous one) and has stopped issuing DMA requests.
    AdcOverrun,
    /// The inputs never stayed within the trigger threshold for 50 ms.
    NotQuiet,
    /// The ADC prescaler did not read back as written.
    ClockConfig,
    /// A sample buffer was already taken ([`Piezo::new`] called twice).
    BufferTaken,
}

/// Result of [`Piezo::idle`].
#[derive(Debug, Clone, Copy)]
pub struct IdleRun {
    /// Per-channel statistics.
    pub stats: IdleStats,
    /// Delivered per-channel sample rate measured against the embassy time
    /// base, in Hz. `None` if the run was a single chunk.
    pub rate_hz: Option<f32>,
}

/// What ended [`Piezo::capture`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum Event {
    /// The trigger fired and the recorder holds a complete window.
    Knock {
        /// The channel that fired the trigger.
        channel: usize,
    },
    /// The user button was pressed while waiting for a trigger.
    Button,
    /// The deadline passed while waiting for a trigger.
    Timeout,
}

/// Debounced button polling for loops that cannot `await` the button because
/// they are busy reading the ADC. Polled once per chunk (10 ms).
pub struct ButtonPoll<'a> {
    button: &'a ExtiInput<'static, Async>,
    released: bool,
    low: u8,
}

impl<'a> ButtonPoll<'a> {
    /// Wrap the (active-low) user button.
    pub fn new(button: &'a ExtiInput<'static, Async>) -> Self {
        Self {
            button,
            released: false,
            low: 0,
        }
    }

    /// True once per press: the button must have been seen released, then
    /// held for about 30 ms.
    pub fn poll(&mut self) -> bool {
        if self.button.is_high() {
            self.released = true;
            self.low = 0;
            return false;
        }
        if !self.released {
            return false;
        }
        self.low += 1;
        if self.low < BUTTON_LOW_CHUNKS {
            return false;
        }
        self.released = false;
        self.low = 0;
        true
    }
}

/// Read each of A0..A3 `samples` times with single blocking conversions and
/// return the averages in counts (ch0..ch3).
///
/// For a quick level check without starting the scan ([`Piezo`]). Uses the
/// longest sampling time (640.5 cycles at 80 MHz = 8 µs), which is enough even
/// for a bare 10 kΩ source. Blocks for roughly `samples × 70 µs`. The ADC is
/// switched off again afterwards.
pub fn read_levels(res: &mut PiezoResources, samples: u16) -> Frame {
    let mut adc = Adc::new(res.adc.reborrow());
    let mut sums = [0u32; CHANNELS];
    for _ in 0..samples.max(1) {
        let time = SampleTime::CYCLES640_5;
        sums[0] += u32::from(adc.blocking_read(&mut res.a0, time));
        sums[1] += u32::from(adc.blocking_read(&mut res.a1, time));
        sums[2] += u32::from(adc.blocking_read(&mut res.a2, time));
        sums[3] += u32::from(adc.blocking_read(&mut res.a3, time));
    }
    let n = u32::from(samples.max(1));
    sums.map(|s| ((s + n / 2) / n) as u16)
}

/// True if ADC1 has flagged a hardware overrun.
fn adc_overrun_flag() -> bool {
    pac::ADC1.isr().read().ovr()
}

/// Divide the ADC clock by [`ADC_PRESCALER`] and calibrate again at that
/// clock. The ADC must be disabled (it is, right after `Adc::new`).
fn set_adc_prescaler() -> Result<(), PiezoError> {
    let ccr = pac::ADC123_COMMON.ccr();
    // CKMODE stays 0: the ADC runs from the RCC kernel clock (SYSCLK), which
    // is the only clock source the prescaler applies to.
    ccr.modify(|w| w.0 = (w.0 & !CCR_PRESC_MASK) | (CCR_PRESC_DIV8 << CCR_PRESC_SHIFT));
    if ccr.read().0 & CCR_PRESC_MASK != CCR_PRESC_DIV8 << CCR_PRESC_SHIFT {
        return Err(PiezoError::ClockConfig);
    }
    // Offset calibration at the new clock (single-ended), as `Adc::new` did
    // at 80 MHz.
    let cr = pac::ADC1.cr();
    cr.modify(|w| w.set_adcal(true));
    while cr.read().adcal() {}
    // At least 4 ADC clock cycles (0.4 µs) must pass before the ADC is
    // enabled; 200 core cycles are 2.5 µs at 80 MHz.
    cortex_m::asm::delay(200);
    Ok(())
}

/// The running piezo acquisition.
pub struct Piezo {
    adc: RingBufferedAdc<'static, peripherals::ADC1>,
    chunk: &'static mut [u16; CHUNK_SAMPLES],
    overruns: u32,
}

impl Piezo {
    /// Configure ADC1 for the four-channel scan at [`SAMPLE_RATE_HZ`] and
    /// start it. Call once.
    pub fn new(res: PiezoResources) -> Result<Self, PiezoError> {
        let ring = cortex_m::singleton!(PIEZO_RING: [u16; RING_SAMPLES] = [0; RING_SAMPLES])
            .ok_or(PiezoError::BufferTaken)?;
        let chunk = cortex_m::singleton!(PIEZO_CHUNK: [u16; CHUNK_SAMPLES] = [0; CHUNK_SAMPLES])
            .ok_or(PiezoError::BufferTaken)?;

        let adc = Adc::new(res.adc);
        set_adc_prescaler()?;

        let sequence = [
            (res.a0.degrade_adc(), SAMPLE_TIME),
            (res.a1.degrade_adc(), SAMPLE_TIME),
            (res.a2.degrade_adc(), SAMPLE_TIME),
            (res.a3.degrade_adc(), SAMPLE_TIME),
        ];
        let mut adc = adc.into_ring_buffered(
            res.adc_dma,
            &mut ring[..],
            PiezoIrqs,
            sequence.into_iter(),
            CONTINUOUS,
        );
        adc.start();
        Ok(Self {
            adc,
            chunk,
            overruns: 0,
        })
    }

    /// Number of [`PiezoError::Overrun`]s reported by [`Piezo::read_chunk`]
    /// so far (not counting backlog discarded by [`Piezo::settle`]).
    pub const fn overruns(&self) -> u32 {
        self.overruns
    }

    /// Discard everything buffered so far; the next chunk starts "now".
    pub fn resync(&mut self) {
        self.adc.clear();
    }

    /// Wait for the next [`CHUNK_SAMPLES`] interleaved samples (10.24 ms).
    ///
    /// The slice starts on a frame boundary: `ch0, ch1, ch2, ch3, ch0, ...`.
    pub async fn read_chunk(&mut self) -> Result<&[u16], PiezoError> {
        match with_timeout(READ_TIMEOUT, self.adc.read(&mut self.chunk[..])).await {
            Ok(Ok(_)) => Ok(&self.chunk[..]),
            Ok(Err(_)) => {
                self.overruns += 1;
                Err(PiezoError::Overrun)
            }
            Err(_) if adc_overrun_flag() => Err(PiezoError::AdcOverrun),
            Err(_) => Err(PiezoError::Stalled),
        }
    }

    /// Measure the idle level: statistics over at least `frames` frames
    /// (rounded up to whole chunks) and the delivered sample rate.
    pub async fn idle(&mut self, frames: u32) -> Result<IdleRun, PiezoError> {
        self.resync();
        let mut stats = IdleStats::new();
        let mut first: Option<Instant> = None;
        let mut timed_frames = 0u32;
        let mut last = Instant::now();
        while stats.frames() < frames {
            let chunk = self.read_chunk().await?;
            last = Instant::now();
            // Chunks complete at DMA half/full-transfer interrupts, so the
            // time between the first and the last completion spans exactly
            // the chunks in between.
            if first.is_none() {
                first = Some(last);
            } else {
                timed_frames += CHUNK_FRAMES as u32;
            }
            stats.push_interleaved(chunk);
        }
        let rate_hz = first.and_then(|t0| {
            let us = last.duration_since(t0).as_micros();
            (timed_frames != 0 && us != 0).then(|| timed_frames as f32 * 1e6 / us as f32)
        });
        Ok(IdleRun { stats, rate_hz })
    }

    /// Get ready for the next knock: discard the backlog, re-centre the
    /// trigger baseline and wait until all channels have stayed within the
    /// threshold for about 50 ms (so the tail of the previous knock does not
    /// fire the trigger again).
    ///
    /// Returns [`PiezoError::NotQuiet`] if that does not happen within 3 s.
    pub async fn settle(&mut self, trigger: &mut Trigger) -> Result<(), PiezoError> {
        self.resync();
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        let mut quiet = 0u8;
        let mut first = true;
        loop {
            let chunk = self.read_chunk().await?;
            if first || trigger.find(chunk).is_some() {
                // Not quiet relative to the old baseline: take this chunk's
                // mean as the new one and start counting again.
                let mut stats = IdleStats::new();
                stats.push_interleaved(chunk);
                trigger.set_baseline(stats.means());
                quiet = 0;
                first = false;
            } else {
                trigger.track(chunk);
                quiet += 1;
                if quiet >= SETTLE_QUIET_CHUNKS {
                    return Ok(());
                }
            }
            if Instant::now() > deadline {
                return Err(PiezoError::NotQuiet);
            }
        }
    }

    /// Arm `recorder` and feed it until the trigger has fired and the window
    /// is complete, the button is pressed, or `deadline` passes.
    ///
    /// While waiting, the trigger baseline follows slow drift. Button and
    /// deadline are only checked before the trigger fires.
    pub async fn capture(
        &mut self,
        trigger: &mut Trigger,
        recorder: &mut Recorder<'_>,
        button: &mut ButtonPoll<'_>,
        deadline: Option<Instant>,
    ) -> Result<Event, PiezoError> {
        recorder.arm(trigger.baseline());
        let mut channel = 0;
        loop {
            let chunk = self.read_chunk().await?;
            let mut quiet = true;
            for frame in piezo::frames(chunk) {
                let hit = if recorder.triggered() {
                    None
                } else {
                    trigger.hit(frame)
                };
                if let Some(ch) = hit {
                    quiet = false;
                    channel = ch;
                }
                if recorder.push(frame, hit.is_some()) {
                    return Ok(Event::Knock { channel });
                }
            }
            if !recorder.triggered() {
                if quiet {
                    trigger.track(chunk);
                }
                if button.poll() {
                    return Ok(Event::Button);
                }
                if deadline.is_some_and(|d| Instant::now() > d) {
                    return Ok(Event::Timeout);
                }
            }
        }
    }
}

/// Send one captured window on the CSV stream of test `test`.
///
/// Sends the stream header again (so a host that attached late still learns
/// the columns), one `# meta ...` line, then one row per entry:
///
/// ```text
/// # <test>,knock,sample,t_us,ch0,ch1,ch2,ch3
/// # meta knock=3,rate_hz=100000,decim=1,pre=100,skew_ns=2500
/// 3,0,-1000,2048,2047,2049,2048
/// ```
///
/// `sample` counts entries from 0; `t_us` is the time of the entry's ch0
/// sample relative to the entry holding the trigger (negative before it).
/// Channel `k` of the same row was sampled `k ×` [`SCAN_SKEW_NS`] later.
/// `decimation` is the recorder's frames-per-entry (with peak hold the values
/// are the extreme of each group, see [`Recorder`]).
pub async fn write_window<W: embedded_io_async::Write>(
    csv: &mut CsvWriter<W>,
    test: &str,
    knock: u16,
    window: &piezo::Window<'_>,
    decimation: usize,
) -> Result<(), CsvError<W::Error>> {
    csv.header(
        test,
        &["knock", "sample", "t_us", "ch0", "ch1", "ch2", "ch3"],
    )
    .await?;
    csv.comment(format_args!(
        "meta knock={},rate_hz={},decim={},pre={},skew_ns={}",
        knock,
        SAMPLE_RATE_HZ,
        decimation,
        window.pre(),
        SCAN_SKEW_NS
    ))
    .await?;
    let step_us = (SAMPLE_PERIOD_US as usize * decimation) as i32;
    for (i, f) in window.frames().enumerate() {
        let t_us = (i as i32 - window.pre() as i32) * step_us;
        csv.row(format_args!(
            "{},{},{},{},{},{},{}",
            knock, i, t_us, f[0], f[1], f[2], f[3]
        ))
        .await?;
    }
    Ok(())
}
