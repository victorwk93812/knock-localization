# Status and known limits of the bench tests

Read this before trusting a number. Update it as tests are run on the board.

**As of 2026-11-24: no test has run on hardware.** All binaries compile, the
driver logic passes its unit tests on a PC, and the capture script passes its
tests on synthetic data.

## Notes on t07 (knock capture)

- **Start the capture script before starting the test.** That is the reliable
  order: every knock ends up in the file. The firmware sends the column header
  (`# t07_capture,knock,sensor,...`) at start-up and again before every knock,
  so a script attached later still records the knocks that follow. It misses
  the knocks before it, and the one being sent while it attached.
- **The file is complete only after the blue button is pressed.** That sends
  `# end`. Without it the script still analyses the run and says so, and it drops
  a knock that was cut off.
- **A missing sensor has no sample rows and the peak `nan`** in the
  `# knock <n> peaks` line. The script treats such a sensor as absent, not as a
  zero reading. Files from the first firmware version have `0` there instead;
  the script still reads them and takes a `0` without sample rows as absent.
- **Trigger: 300 mg** of dynamic acceleration on any sensor; a first guess, to be
  set from the t06 noise numbers.
- **Window:** 4 samples before and 16 after the trigger (−10 ms … +40 ms at
  400 Hz). The windows of different sensors are aligned to about ±1 sample
  (±2.5 ms), because the sensors' FIFO data carries no timestamps. That is fine
  for peak amplitudes and useless for arrival times; arrival times come from the
  piezos (t10).
- **At least 300 ms between knocks.** Faster knocks are merged.

## Notes on t09 (piezo front end)

- **Sampling is 100 000 samples/s per channel, exactly**, derived from the ADC
  clock (80 MHz ÷ 8, 25 clock cycles per conversion, 4 channels). The register
  setting for the ÷ 8 is from memory of ST's reference manual and is not modelled
  by the HAL. **t09 measures the delivered rate and fails if it is off by more
  than 1 %**, so a wrong setting cannot go unnoticed.
- **A hard knock reaches the ADC limits (0 or 4095) by design.** The diode clamps
  act at about −0.65 V and 3.95 V, just outside the ADC's 0–3.3 V range, so the
  ADC saturates before the clamps do. t09 reports this as a warning, not a
  failure. A flat top below the rails would mean something else is limiting.
- **Check the pin voltage limit before connecting A0–A3.** 3.95 V on a 3.3 V pin
  has to be within the STM32L475 datasheet's absolute maximum ratings (input
  voltage and injected current for analog pins). Not yet checked: the datasheet
  is not in `references/`.
- **Thresholds are first guesses:** idle level 2048 ± 150 counts, idle noise
  below 8 counts RMS, knock trigger 60 counts from the baseline, flat-top = 5
  consecutive samples within 3 counts of the extreme. They will be set from the
  first real log.
- **Knocks during the data dump are ignored.** Each knock takes about 0.4 s to
  send (LED2 is off meanwhile). Wait for LED2 before the next knock.
- **The channels are sampled 2.5 µs apart**, one after the other. t10 corrects
  its onset times for this; raw CSV rows are not corrected.

## First-guess thresholds (all tests)

| Test | Value | Set it from |
|---|---|---|
| t05, t07 | knock trigger 300 mg | t06 on the mounted plate (5 × the median noise) |
| t05 | saturation flag at 15.5 g on any axis | fixed (full scale is 16 g) |
| t06 | sensor flagged when noisier than 3 × the median | experience |
| t09, t10 | trigger 60 counts; idle RMS limit 8 counts | first t09 log |
| t10 | onset = 8 σ of the pre-trigger noise, at least 6 counts | first t10 log |
| t05, t07, t10 | 300 ms between knocks | t10 long window (T11) |

## Values not checked against a primary source

The ST documents are not in `references/` yet (LSM6DSL datasheet, UM2153 board
manual, STM32L475 datasheet).

- ADC prescaler bits and conversion time (affects the piezo sample rate; t09
  checks it).
- ADC input figures used to justify the short sampling time (5 pF, 1 kΩ).
- STM32L475 absolute maximum pin voltage against the 3.95 V clamp.
- LSM6DSL registers and its sensitivity at ±16 g (0.488 mg/LSB), used for the
  table reference in t00 and t06.
- Arduino header nets (D14/D15, A0–A3, D2 = PD14) are taken from the Zephyr board
  files, not from UM2153. D2 only matters for the optional logic-analyzer pin in
  t04.
- BMI160 wait after soft reset (10 ms, chosen conservatively).
- BMI160 "FIFO full" taken as a fill level of 1000 bytes or more.

In the sources these places are marked `UNVERIFIED`.

## Known limits

- **t06:** the suggested trigger threshold is 5 × the median noise, so it
  describes the typical sensor. A sensor noisier than that can trigger by
  itself at this threshold. t06 prints what the noisiest sensor would need;
  if the two numbers are far apart, fix that sensor before using the threshold.
- **t13:** the multiplexer's RESET pin is not wired. If a sensor holds the data
  line low even after bus recovery, every sensor is reported down until that
  sensor is unplugged or the array is power-cycled.
- **t13:** the watchdog also runs while a debugger halts the program; a halt
  longer than 2 s resets the board.
- **100 kHz fallback:** t04 and t13 cannot pass at 100 kHz (the bus is too slow
  to drain nine sensors every 20 ms).
- **Grid points at (±4, ±4) cm lie on the piezo discs.** Knock 1 cm beside them.
