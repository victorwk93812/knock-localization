# Material checks and preparation

Checks that need only the multimeter, and the soldering that has to happen before
the firmware tests. Do the checks with the power off and the parts out of circuit.
Results go in [CHECKLIST.md](CHECKLIST.md), milestone M1.

## 1. Tools and board

| Item | Test | Pass |
|---|---|---|
| Multimeter | Continuity mode, touch the probes together | Beep, < 0.5 Ω |
| USB cable | Plug in the board. Linux: `lsusb`. Windows: Device Manager. macOS: System Information → USB | An ST-LINK device appears (`0483:374b` in `lsusb`). Only an LED and no device = charge-only cable |
| Board power | DC volts between the header pins 3V3 and GND, then 5V and GND | 3.25–3.35 V, about 5 V |

## 2. Passive parts

| Part | How | Pass |
|---|---|---|
| Resistors 10 kΩ, 100 kΩ, 1 MΩ | Ω mode. Don't hold both leads of the 1 MΩ: your fingers read in parallel | Within ±1 % (metal film) |
| 100 nF (`104`), 1 nF (`102`) | Capacitance mode; zero the meter with open leads first | 100 nF within ±20 %. 1 nF is hard to read on cheap meters; the printed code is enough |
| 10 µF electrolytic | Capacitance mode. The stripe marks the **negative** lead | 8–12 µF |
| 1N4148 diodes | Diode mode. Red probe on the plain end, black on the banded end; then swap | **0.55–0.70 V** forward, `OL` reversed. About 0.2–0.3 V would be a Schottky: wrong part |

## 3. Wires and breadboard

- **Every Dupont wire:** continuity end to end (< 1 Ω), then bend the wire next to
  each crimp while the meter beeps. A stutter means a bad crimp: bin it.
- **Female ends:** push each onto a header pin. A loose socket causes random I2C
  errors later (T3, T13). Bin it.
- **Breadboard:** each 5-hole row conducts; neighboring rows don't. Check the power
  rails end to end and bridge them if they are split in the middle.

## 4. GY-BMI160 modules (all 10)

### 4.1 Before soldering

1. **Read the chip marking** with a phone macro photo. Note anything that isn't a
   BMI160.
2. **Short check:** Ω between VIN and GND, and between 3V3 and GND. Neither may be
   near 0.
3. **Pull-ups:** Ω from SDA to 3V3 and from SCL to 3V3. Write the value down; it
   helps to diagnose T3. Each multiplexer channel sees only its own module's
   pull-ups; the trunk sees those of module 8 plus the multiplexer board's.

### 4.2 What the pins do

The names are printed on the module. The order differs between makers, so go by
the printed name, not by position.

| Pin | Function | In this project |
|---|---|---|
| VIN | Supply into the module's small regulator | 3.3 V from the board (see §4.4) |
| 3V3 | Regulator output = the chip's supply | Normally left alone; strap target for CS and SAO |
| GND | Ground | Ground |
| SCL | I2C clock | To D15, or to a multiplexer `SCk` |
| SDA | I2C data | To D14, or to a multiplexer `SDk` |
| CS | Chip select. Low at power-up puts the chip in SPI mode | **Strapped to 3V3** |
| SAO | Lowest bit of the I2C address | **GND** → 0x68 (modules 0–7, 9). **3V3** → 0x69 (module 8) |
| INT1, INT2, OCS, SCX, SDX | Interrupts and auxiliary bus | Not connected |

### 4.3 Soldering

The module will be glued flat on the acrylic, back side down, so nothing may stick
out underneath.

**Option A: pin header (easier to rewire).**

1. Break off a header strip as long as the module's row of holes.
2. Insert it from the **component side**: plastic body and long pins on the
   component side, short tails through to the back. Plugging the long pins into
   a breadboard holds everything square, with the module's back facing up.
3. Solder each tail on the back: iron on pad and pin together for about 2 s, feed
   a little solder, remove. Keep the joints small; a good one is a shiny cone.
4. Trim the tails and the solder peaks as flat as flush cutters allow (wear
   glasses; the offcuts fly). What remains should be under about 0.5 mm, which
   the glue layer takes up. If you want a truly flat back, use option B.
5. Check with the meter that neighboring pins are not bridged.

**Option B: wires soldered directly (lowest mass, flattest).** Strip 2 mm, tin the
wire, push it in from the component side, solder, trim flush underneath. Use four
colors consistently: red = supply, black = GND, yellow = SCL, blue = SDA (or any
scheme, written down).

**Straps, on every module, with short pieces of wire:**

- **CS → 3V3.** A link between the CS pin and the module's 3V3 pin.
- **SAO → GND** on modules 0–7 and 9. **SAO → 3V3** on module 8.
- Solder the links on the component side so the back stays flat. If CS/SAO and
  3V3/GND are not adjacent, use a thin insulated wire.
- Check each strap with the meter: CS to 3V3 < 1 Ω; SAO to GND (or 3V3) < 1 Ω;
  3V3 to GND still not shorted.

With the straps on the module, only four wires run to each sensor.

### 4.4 Supply check and labels

1. Feed 3.3 V into VIN and measure the module's **3V3 pin**. The regulator loses a
   little. If the pin reads below about 3.0 V, feed 3.3 V into the 3V3 pin
   instead and leave VIN open.
2. **Label** each module 0–9 with tape. Module 8 is the one strapped to 0x69.

Then run [tests/t01_chip_id.md](tests/t01_chip_id.md) on every module.

## 5. Piezo discs (all 6)

### 5.1 Soldering the leads

The disc has two electrodes: the **brass plate** (the large outer area) and the
**silver ceramic** disc in the middle of one face. The other face is plain brass
and is the face that gets glued to the plate.

1. Cut a red and a black lead, about 40 cm each (26 AWG stranded). Strip 3 mm and
   tin both ends.
2. **Black → brass rim**, on the same face as the ceramic, near the edge. Brass
   takes heat well: 2–3 s.
3. **Red → silver ceramic**, near its edge, not in the middle. Put a small dot of
   solder on the silver first, then reflow the tinned wire onto it. **Keep it
   under 2 s and keep the blob small.** Too much heat lifts the silver or
   depolarises the ceramic, and a heavy blob changes the disc's response.
4. Twist the two leads together along their length (less pickup).
5. Tin the free ends, or solder them to header pins, so they hold in the
   breadboard.
6. Label the discs 0–5.

### 5.2 Checks

| Check | Pass |
|---|---|
| Ω between the leads | Open (> 10 MΩ). Near 0 = shorted or cracked |
| Capacitance | Roughly 10–30 nF, similar on all discs. Near 0 = open joint or cracked ceramic |
| DC volts, tap the disc with a fingernail | The reading jumps |
| Tug each lead gently | The joint holds |

Then build the front end and run [tests/t09_piezo_dc.md](tests/t09_piezo_dc.md).
