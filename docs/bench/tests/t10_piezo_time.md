# t10_piezo_time — T10 and T11: arrival times and ring-down

**Proves (T10):** the piezo nearest to a knock fires first, a center knock arrives
at all four at nearly the same time, and the time differences give a wave speed.
**Measures (T11, long window):** how long the plate rings after a knock, which
sets the minimum time between knocks.

## You need

- t09 passing.
- The four discs **glued to the plate** at their positions, brass face down, and
  the plate on its feet. Positions in cm from the plate center:
  P0 (−5, +3.33), P1 (+5, +3.33), P2 (+5, −3.33), P3 (−5, −3.33); P0 on A0, P1
  on A1, and so on.
- The paper template under the plate.

## Wiring

Same as [t09](t09_piezo_dc.md). The accelerometers may stay connected; this test
ignores them.

## Steps — T10

1. Terminal 1, from `host/`, **first**:

   ```sh
   uv run bench/capture.py --port /dev/ttyACM0 --out ../data/bench
   ```
2. Terminal 2, from `firmware/`:

   ```sh
   cargo run --release --bin t10_piezo_time
   ```
3. LED2 on = armed. Knock **on the plate right beside P0**, 3 times. After each
   knock wait for LED2 to come back on (about 0.4 s). Then P1, P2, P3, in that
   order, so you know which knock numbers belong to which disc.
4. Hold the blue button half a second. Stop the script. This is file 1.
5. Start script and test again; knock **at the plate center** 10 times; finish
   the same way. This is file 2. (The center is where I8 sits: knock directly
   beside it, always on the same spot.)
6. **Wave speed, file 3.** A center knock is equally far from all four discs, so
   it cannot give a speed. Record one more file with 10 knocks at a single marked
   off-center point, for example (−10, 0) cm, then:

   ```sh
   uv run bench/capture.py --replay ../data/bench/<file3>.csv --known-pos=-10,0
   ```

   Write negative coordinates with `=` as shown. Every knock in the file must be
   at that one point; the script applies the position to all of them.

## Steps — T11 (ring-down)

```sh
# terminal 1, host/ — first
uv run bench/capture.py --port /dev/ttyACM0 --out ../data/bench
# terminal 2, firmware/
cargo run --release --bin t10_piezo_time --features knock-bench/long-window
```

Knock a few times at different places, including a hard knock. Each capture now
covers 500 ms and takes about 0.8 s to send. Finish with the blue button.

## What you get

Per knock, in the log and from the script:

```text
onset times [us] relative to the trigger, and relative to the earliest channel
knock   src    t0   t1    t2    t3   dt0  dt1   dt2   dt3        order
    1    fw  76.6  5.0  34.4  93.6  71.6  0.0  29.4  88.6  P1 P2 P0 P3
```

`order` lists the discs from first to last arrival. With `--known-pos` the script
adds `median v_eff = … m/s`. In long-window mode it prints the ring-down per
channel in ms and `longest ring-down … ms: refractory period must be longer`.

The onset of a channel is the first sample that leaves that channel's own
pre-knock noise band (8 σ, at least 6 counts), interpolated between samples and
corrected for the 2.5 µs between channels.

## Pass

**T10** (you judge these; the firmware cannot know where you knocked):

- Knocks beside P*k*: P*k* is first in `order`, every time.
- Center knocks: the four onsets lie within about 100 µs of each other.
- A wave-speed estimate that is stable from knock to knock.

The firmware's own `RESULT` only checks that at least one knock was captured,
every knock gave an onset on all four channels, and no samples were lost.

**T11:** every channel settles within the 500 ms window (no `inf`). The refractory
period is the longest ring-down plus a margin.

## Write down

Arrival order per disc; center spread in µs; v_eff; longest ring-down; file names.

## If it fails

| Symptom | Likely cause |
|---|---|
| The wrong disc is first | Wires to A0–A3 crossed, or the discs are not at the positions listed. Tap each disc and watch which channel peaks (t09) |
| One channel has no onset (`nan`) | Weak signal on that disc: poor glue joint or cracked disc. Compare peaks between channels |
| One channel always late by a similar amount | That disc is poorly coupled (thick glue, air bubble) |
| Center knocks are not symmetric | You are not knocking at the center (a knock 1–2 cm off already shifts the arrivals measurably), or one disc is misplaced |
| Speed estimates scatter widely | Knock position not repeatable, or onsets found on noise: look at the saved plots |
| Ring-down `inf` | Still ringing after 500 ms: check that the plate stands on all four feet and no cable rests on it |
| `samples lost while armed` | Report it; the firmware could not keep up |
