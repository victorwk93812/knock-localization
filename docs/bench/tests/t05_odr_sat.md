# t05_odr_sat — T5: sample rate and saturation

**Proves:** every sensor really delivers 400 samples/s, and the hardest knock you
will ever use stays inside the ±16 g range.

## You need

The full array; t04 passing. The **rate check** (phase 1) works on the
breadboard. The **knock check** (phase 2) only means something with the modules
mounted on acrylic, so run this test twice: now for phase 1, and again after
assembly for phase 2.

## Wiring

Same as [t02](t02_mux_scan.md).

## Steps

1. From `firmware/`:

   ```sh
   cargo run --release --bin t05_odr_sat
   ```
2. **Phase 1, 10 s: don't touch anything.** The firmware counts the samples each
   sensor delivers.
3. **Phase 2 starts when LED2 turns on.** Knock right next to each module, as
   hard as you ever would in use, a few times each. Leave at least half a second
   between knocks; knocks closer than 300 ms count as one.
   On the breadboard set-up, just tap one module so that at least one knock is
   registered.
4. Press the **blue button** to finish.

## What the log shows

Phase 1, per sensor:

```text
I3: 400.6 frames/s (4006 frames in 10000 ms; expected 400 +-2 %)
```

Phase 2, per knock, the peak |a| of every sensor in g (gravity included):

```text
INFO: knock 4: peak |a| in g: I0 1.02 I1 1.10 I2 3.85 I3 9.71 ...
```

and `knock N: Ik SATURATED` if any axis reached 15.5 g. The last lines give the
number of knocks and the worst peak.

## Pass

- Every sensor between 392 and 408 samples/s.
- At least one knock detected.
- No axis saturated.

## Write down

The rate range across sensors; the worst peak in g and on which sensor.

## If it fails

| Symptom | Likely cause |
|---|---|
| One sensor's rate is off | That module's oscillator is out of tolerance: swap in the spare |
| All rates off by the same amount | The board's clock, not the sensors. Report the numbers |
| No knock detected | Trigger is 300 mg of dynamic acceleration. On loose modules a tap on the table may not reach it: tap the module itself |
| Saturation | Knock softer and make that the rule for the dataset, or note it as a limit in the report. The range is already at its maximum |
