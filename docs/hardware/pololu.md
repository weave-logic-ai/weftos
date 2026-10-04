# Pololu — WeftOS/WeaveLogic hardware KB

Public hardware facts for [Pololu](https://www.pololu.com) parts relevant to our
WeftOS / Cognitum sensing, power, and actuation story. Pololu is a US
robotics/electronics vendor (Las Vegas, NV) whose signature strengths are
**switching voltage regulators**, **motor drivers and controllers**, and
**sensor carrier boards** that break ST / Allegro / TI / Bosch chips onto 0.1″
breadboard-friendly PCBs with onboard regulation and level-shifting. Many of
those carriers expose the *same chips we already drive* through DFRobot/Gravity
or the Seed's native drivers — so they are drop-in alternatives for our cogs.

Specs are vendor-claimed (Pololu category/product pages); each family cites its
source URL. Numbers marked *(verify)* are newer/edge items to confirm against
the live product page before quoting. No appliance credentials or
partner-internal details here — hardware only.

Lead families for us, in order: **sensor carriers** → **power / regulators** →
**motor & servo control** → **robot platforms / tools**.

---

## 1. Sensor carrier boards (HIGH relevance)

Pololu carriers put a known sensor IC on a 0.1″-pitch board with a 2.8 V (or
3.3 V) regulator + level shifters, so most run on a **2.6–5.5 V** logic/supply
range and talk **I²C** (several also SPI). Where a carrier exposes a chip
already in our catalog, it's a drop-in alternative to the Gravity/Seed path.

### Time-of-Flight (ToF) distance — ST VLxxx carriers

Our 8×8 ToF cog (`sen0628-tof`) is a DFRobot board wrapping ST's **VL53L7CX**;
Pololu sells carriers for the whole VLxxx line, including direct siblings of
that chip. All are I²C, 940 nm eye-safe VCSEL, onboard 2.8 V regulator +
level-shifters, **2.6–5.5 V** input, 0.1″ pins.

| Carrier | Chip | Max range | Zones / FOV | Overlap with our catalog |
|---|---|---|---|---|
| VL6180X | VL6180X | 60 cm | single + ambient light | — (adds lux channel) |
| VL53L0X | VL53L0X | 200 cm | single | classic low-footprint |
| VL53L4CD | VL53L4CD | 120 cm | single, ultra-low-power | — |
| VL53L1X | VL53L1X | 400 cm | single, programmable ROI | **= our VL53L1X candidate** (sensors.md) |
| VL53L3CX | VL53L3CX | ~500 cm *(verify)* | single, multi-target | — |
| VL53L5CX | VL53L5CX | 400 cm | **8×8** (or 4×4) | **sibling of SEN0628's VL53L7CX** |
| VL53L7CX | VL53L7CX | ~350 cm *(verify)* | **8×8**, 90° wide FOV | **= the exact chip in our `sen0628-tof` cog** |
| VL53L8CX | VL53L8CX | 400 cm | **8×8**, newer gen | drop-in 8×8 upgrade path |

- **Drop-in note**: `sen0628-tof` reads a DFRobot board that fronts the VL53L7CX
  behind an RP2040. Pololu's **VL53L7CX / VL53L5CX / VL53L8CX** carriers expose
  the raw ST sensor directly over I²C (no RP2040 shim) — a leaner, host-driven
  alternative if we want to own the multizone housekeeping. **VL53L1X** carrier
  = the single-zone sibling already tracked as a candidate in `sensors.md`.
- **Gotchas**: one I²C address out of reset on most ST ToF parts → multi-sensor
  buses need XSHUT sequencing to reassign addresses at boot. 8×8 modes (L5/L7/L8)
  need more host memory and run slower than 4×4. Ambient IR and cover-glass
  crosstalk degrade range.
- **Source**: <https://www.pololu.com/category/306/carriers-for-st-time-of-flight-tof-distance-sensors>

Also in this family: **3-Channel Wide FOV ToF (OPT3101)** — ~1 m range, ~180°
combined FOV, I²C — and Pololu's own **digital distance sensors** (lidar-based,
simple digital high/low output), **Sharp GP2Y** analog IR distance sensors, and
**MaxBotix / Parallax** ultrasonic range finders.
Source: <https://www.pololu.com/category/189/proximity-sensors-and-range-finders>

### IMU / AHRS — ST MEMS carriers

These expose the **exact ST MEMS parts** in our broadline manufacturer catalog
(STMicroelectronics IMUs). All 0.1″ pins, I²C (most also SPI), **2.5–5.5 V**
(LSM6DSO carrier down to 1.8 V).

| Board | Chips | DOF | Interface |
|---|---|---|---|
| **AltIMU-10 v6** | LSM6DSO (accel+gyro) + LIS3MDL (mag) + LPS22DF (baro) | 10 | I²C |
| **MinIMU-9 v6** | LSM6DSO + LIS3MDL | 9 | I²C |
| MinIMU-9 v5 | LSM6DS33 + LIS3MDL | 9 | I²C/SPI |
| LSM6DSO carrier | LSM6DSO | 6 | I²C/I3C/SPI |
| LSM6DS33 carrier | LSM6DS33 | 6 | I²C/SPI |
| LIS3MDL carrier | LIS3MDL (mag only) | 3 | I²C/SPI |
| L3GD20H / L3GD20 carrier | gyro only | 3 | I²C/SPI |
| UM7 / UM7-LT | fused 9-DOF orientation module (serial/SPI) | 9 | serial/SPI |

- **Fit**: a drop-in orientation/tilt cog. The **AltIMU-10** gives accel + gyro +
  mag + barometric pressure in one board — a richer, host-fused alternative to
  the fusion-on-chip BNO055 candidate in `sensors.md` (BNO055 does the fusion
  itself; the ST stack needs host-side fusion math, e.g. Madgwick/Mahony). The
  LPS22DF pressure channel overlaps the BME280's pressure/altitude role.
- **Gotchas**: magnetometer needs figure-8 calibration and is thrown by nearby
  metal/magnets; no on-chip absolute-orientation fusion (unlike BNO055) — you own
  the filter. UM7 is a serial orientation *module*, not a bare-chip carrier.
- **Source**: <https://www.pololu.com/category/80/accelerometers-gyros-compasses>

### Current sensors (fleet-node power monitoring)

Signature Pololu strength for measuring node/actuator current. Mostly Allegro
Hall-effect (galvanically isolated, analog out); the ACS37800 adds isolated I²C
power metering.

| Carrier | Chip | Range | Dir | Output | Supply |
|---|---|---|---|---|---|
| ACS711 | Allegro ACS711 | ±15.5 / ±31 A | bi | analog | 3.0–5.5 V |
| ACS724 / ACS723 | Allegro ACS724 | ±2.5…±50 A | bi/uni | analog (differential) | 4.5–5.5 V |
| ACS72981 | Allegro ACS72981 | ±50…±200 A | bi | analog | 3.3 / 5 V |
| ACS37800 | Allegro ACS37800 | ±30 A | bi | **I²C** (isolated power monitor) | 3.3–5.5 V |

- **Fit**: a current/power-telemetry cog for fleet-node and actuator rails. The
  **ACS37800** (I²C, isolated V+I+power) is the natural digital-telemetry pick;
  the Allegro Hall parts are galvanically isolated, unlike a shunt+INA solution.
- **Gotchas**: analog ACS outputs need an ADC (e.g. the ADS1115 we already use
  for the ECG path) and ratiometric reference handling. Pick the range to bracket
  expected current without losing resolution. Pololu's line is Allegro Hall-based,
  **not** TI INA219/INA226 shunt monitors — if you specifically want a shunt
  I²C monitor, that's an Adafruit/other-vendor part, not Pololu.
- **Source**: <https://www.pololu.com/category/118/current-sensors>

### Reflectance / line / encoders / environmental

- **QTR reflectance sensors** — IR LED + phototransistor arrays (line following,
  edge/surface detection), analog or RC-timed digital output.
  <https://www.pololu.com/category/123/pololu-qtr-reflectance-sensors>
- **Encoders** — magnetic + optical quadrature encoders for motor speed/position
  feedback (pairs with the gearmotors below).
  <https://www.pololu.com/category/201/encoders>
- **Environmental sensors** — barometric pressure, humidity, temperature,
  luminosity carriers (overlaps the BME280 role).
  <https://www.pololu.com/category/190/environmental-sensors>
- **Force-sensing resistors & linear pots** — analog force / position.
- **38 kHz IR proximity sensors** (Pololu) + Vishay IR detector modules —
  simple presence detection (30–60 cm).

---

## 2. Power / voltage regulators (HIGH relevance)

Pololu's flagship category and the most directly useful to the Fleet-node power
story: compact, high-efficiency switching regulators with wide input ranges —
exactly what a battery- or PoE-fed sensor node needs. All are vendor-claimed
specs; confirm the exact family variant on the product page.

### Step-down (buck)

Efficiently generate a lower output from a higher input. Families span tiny
600 mA boards up to 15 A.

| Family | Output | Input (max) | Max current |
|---|---|---|---|
| D24V150Fx | 3.3–12 V | up to 40 V | **15 A**, 80–95% eff. |
| D24V10Fx | fixed (3.3/5/…) | up to 36 V | 1 A, very low dropout |
| D24Vxx / lower families | fixed/adjustable | up to 50 V *(verify)* | 600 mA–several A |

Source: <https://www.pololu.com/category/131/step-down-buck-voltage-regulators>

### Step-up (boost)

Raise a lower input (even sub-1 V) to a higher regulated output — for running
5 V logic from 1–2 cells, etc.

| Family | Output | Input | Max input current |
|---|---|---|---|
| U3V70x | 3.3–24 V adj. | 4.5–20 V | 10 A |
| U3V50x | 3.3–9 V (adj. 4–30 V) | ≥2.9 V | 5 A |
| U3V40x | 3.3–9 V | 2.7 V start / 1.3 V run | 9.5 A |
| U3V16x | 3.3–18 V | 2.7 V start / 1.3 V run | 2 A |
| U1V11x / U1V10x | 3.3 / 5 V (adj. 2–5.25 V) | **≥0.5 V** | 1.2 A |

Source: <https://www.pololu.com/category/132/step-up-boost-voltage-regulators>

### Step-up/step-down (buck-boost) — best for batteries

Regulate across inputs above *and* below the target — ideal when a battery
starts above the output and sags below it as it discharges.

| Family | Output | Input | Max current |
|---|---|---|---|
| S13VxFx | 3.3–15 V | 2.8–22 V | 1–3 A (by variant) |
| S9V11x | 2.5–9 V | 2–16 V | buck-boost |
| S7V8x (e.g. S7V8A) | 2.5–8 V adj. | 2.7–11.8 V | >90% eff. |

Source: <https://www.pololu.com/category/133/step-up-step-down-voltage-regulators>
· top: <https://www.pololu.com/category/136/voltage-regulators>

### Power switches / load switches / protection

| Product | Function | Rating |
|---|---|---|
| Mini MOSFET Slide Switch, **LV** | high-side P-FET switch + reverse-voltage protection | ~6 A, **1.8–20 V** (only one <4.5 V; single Li-cell OK) |
| Mini MOSFET Slide Switch, **SV** | same | ~4 A, 4.5–40 V |
| Big MOSFET Slide Switch, **MP / HP** | higher-power switch + reverse protection | tens of A *(verify)* |
| Pushbutton power switches | latching soft on/off via pushbutton | several variants |
| Reverse-voltage protectors / ideal-diode | block reverse current / OR-ing | by variant |

- **Fit**: the MOSFET slide switches give a node a clean soft-power switch with
  **reverse-voltage protection** (main current bypasses the mechanical switch, so
  a tiny switch controls big current). The LV is the pick for single-Li-cell
  nodes (works down to 1.8 V). MCU can drive the gate instead of the slide switch.
- **Source**: <https://www.pololu.com/product/2810> (LV) · <https://www.pololu.com/product/2811> (SV) · <https://www.pololu.com/product/2814> (Big MP)

---

## 3. Motors & actuators (NEW territory — the actuator-cog angle)

This is new ground for the KB: the output/actuation side of a sensing+acting
cog. Pololu covers the whole stack from bare driver ICs up to USB-smart
controllers.

### Stepper motor driver carriers (bare driver IC)

Low-level chip carriers — host supplies STEP/DIR, driver handles current
chopping + microstepping. All vendor-claimed continuous current (higher with
cooling).

| Carrier | Chip | A/phase (cont.) | Motor V | Microstep |
|---|---|---|---|---|
| A4988 | Allegro A4988 | 1.0 A | 8–35 V | 1/16 |
| A5984 | Allegro A5984 | 1.2 A | 8–40 V | 1/32 |
| DRV8825 | TI DRV8825 | 1.5 A | 8.2–45 V | 1/32 |
| DRV8834 | TI DRV8834 | 1.5 A | 2.5–10.8 V | 1/32 |
| DRV8434 | TI DRV8434 | 1.2 A | 4.5–48 V | **1/256** |
| MP6500 | MPS MP6500 | 1.2 A | 4.5–35 V | 1/8 |
| MP6602 / MP6603 | MPS | 1.5 / 2.2 A | 4.5–35 / 8–55 V | 1/32 / 1/8 |
| TB67S249 / S279 / S579 / S581 / S128 | Toshiba | 1.1–2.1 A | 4.5–47 V | 1/32–1/128 |
| STSPIN220 / STSPIN820 | STMicro | 1.1 / 1.2 A | 1.8–10 / 7–45 V | **1/256** |

Source: <https://www.pololu.com/category/120/stepper-motor-drivers>

### Brushed DC motor driver carriers

| Carrier | Chip | Channels | Current | Motor V |
|---|---|---|---|---|
| DRV8833 | TI DRV8833 | 2 | 1.2 A cont. (2 A pk) /ch | 2.7–10.8 V |
| DRV8835 | TI DRV8835 | 2 | low-power dual | 2–11 V |
| TB6612FNG | Toshiba | 2 (or 1 bipolar stepper) | ~1 A/ch | up to ~13.5 V |
| DRV8838 | TI | 1 | ~1.7 A | 0–11 V |
| DRV8256E/P | TI DRV8256 | 1 | 1.9 A cont. (6.4 A pk) | 4.5–48 V |
| MP6550 | MPS | 1 | 1.7 A cont. (2.5 A pk) | 1.8–22 V |
| A4990 | Allegro | 2 | dual | up to 32 V |

Source: <https://www.pololu.com/category/11/brushed-dc-motor-drivers>

### Smart motor controllers (USB / I²C / serial — the cog-friendly tier)

These are the ones that map cleanly to an **actuator cog**: a microcontroller
on-board accepts USB / TTL-serial / I²C / RC / analog commands, so the Seed or a
Fleet node drives them over a bus instead of bit-banging STEP/DIR.

- **Tic stepper controllers** (T500, T834, T825, T249, **36v4**) — USB + TTL
  serial + I²C + RC + analog + quadrature-encoder input; drive a bipolar stepper
  with speed/position targets. T500=MP6500, T834=DRV8834 (2.5–10.8 V), T825=DRV8825
  (8.5–45 V), T249=TB67S249, 36v4=discrete-MOSFET H-bridge for large motors.
  <https://www.pololu.com/category/212/tic-stepper-motor-controllers>
- **Motoron** (22 models, 1–3 channels) — **I²C / UART** brushed-DC controllers;
  stackable, well-suited to multi-motor nodes over a single bus.
- **Jrk G2** (5 models) — single-channel brushed DC **with feedback** (closed-loop
  speed/position), USB/serial/I²C/RC/analog.
- **Simple Motor Controllers G2** (4 models) — single-channel, USB/TTL/analog/RC.
- **RoboClaw** (10 models, 1–2 ch, up to ~300 A) and **MCP** (2-ch, RS-232/CAN) —
  high-power third-party controllers Pololu stocks.
- **Qik** dual serial — legacy 2-channel serial.
- Source: <https://www.pololu.com/category/10/power-and-motion> (brushed DC controllers)

### Maestro servo controllers

The go-to for coordinated hobby-servo motion — a strong actuator-cog substrate.

| Model | Channels | Interfaces | Resolution | I/O notes |
|---|---|---|---|---|
| Micro Maestro 6 | 6 | USB / TTL serial / I²C | 0.25 µs | 6 analog-in capable |
| Mini Maestro 12 | 12 | USB / TTL serial / I²C | 0.25 µs | 12 analog-in |
| Mini Maestro 18 | 18 | USB / TTL serial / I²C | 0.25 µs | 6 digital-in, 12 analog-in |
| Mini Maestro 24 | 24 | USB / TTL serial / I²C | 0.25 µs | 12 digital-in, 12 analog-in |

- **Fit**: channels are individually servo / digital-out / analog-in; built-in
  scripting + per-servo speed/accel. A Seed can drive it over I²C or USB. Ideal
  as the motion back-end of a pan/tilt or multi-servo actuator cog.
- **Source**: <https://www.pololu.com/category/102/maestro-usb-servo-controllers>

### Gearmotors, steppers, servos (hardware)

Micro-metal gearmotors, plastic gearmotors, 37 D-mm metal gearmotors (many with
integrated quadrature encoders), stepper motors (NEMA 8/11/14/17/23), and a
hobby/sub-micro servo range — the physical actuators the drivers/controllers
above turn. (Mounting brackets + wheels + hubs round out the mechanical set.)

---

## 4. Robot platforms / dev boards / tools (brief)

- **3pi+ 2040 / 32U4** — complete small robots (RP2040 or ATmega32U4 brain,
  dual gearmotors + encoders, line/IMU sensors). Good self-contained mobile cog.
- **Zumo 32U4 / 2040** — tracked sumo-style robot platform.
- **Romi / Balboa** — Romi chassis (differential-drive round base) and Balboa
  balancing robot; A-Star 32U4 control boards slot onto them.
- **A-Star** — Arduino-compatible ATmega32U4 / SB boards (standalone or chassis).
- **Programmers/tools** — **USB AVR Programmer v2.1** (also a USB-TTL serial
  adapter), USB-to-serial adapters, logic/level shifters, and bench accessories.
- Platforms index: <https://www.pololu.com/category/1/robots-and-kits>

---

## Cross-cutting notes for our stack

- **Overlaps with chips we already drive**: Pololu **VL53L7CX** carrier = the
  exact ToF imager inside our `sen0628-tof` cog (DFRobot SEN0628); **VL53L5CX /
  VL53L8CX** are 8×8 siblings; **VL53L1X** carrier = the single-zone candidate in
  `sensors.md`. The **AltIMU-10 / MinIMU-9** expose the ST LSM6DSO/LIS3MDL/LPS22
  parts from our broadline ST catalog. Current-sensor carriers pair with the
  **ADS1115** we already use for analog capture.
- **Why Pololu for us specifically**: the regulator + MOSFET-switch + current-
  sensor trio is a ready-made **fleet-node power front-end** (wide-input buck/boost,
  soft power + reverse-protection, isolated current telemetry). The Tic / Motoron /
  Maestro smart controllers are the cleanest path to an **actuator cog** — they
  take I²C/serial commands, so a Seed drives them over a bus with no real-time
  STEP/DIR burden on the host.
- **Honesty flags**: regulator current ceilings are best-case (thermal-limited);
  ToF ranges assume high-reflectivity targets under favorable ambient IR; ST ToF
  carriers share one I²C address out of reset (XSHUT sequencing for multi-sensor
  buses); Pololu current sensors are Allegro **Hall-effect** (isolated, analog),
  **not** TI INA2xx shunt monitors.
- **Source grounding**: all specs above are from pololu.com category/product
  pages (fetched 2026-10-04); confirm the exact variant on the live page before
  quoting a number, especially items marked *(verify)*.

## Standout parts worth adding to the sensor-explorer pool later

- **VL53L7CX / VL53L5CX / VL53L8CX** ToF 8×8 carriers — direct bare-chip
  alternatives to our SEN0628 ToF cog.
- **VL53L1X** single-zone ToF carrier — lightweight presence/distance sibling.
- **AltIMU-10 v6** (LSM6DSO + LIS3MDL + LPS22DF) — one-board 10-DOF with pressure.
- **ACS37800** I²C isolated power monitor — digital current/power telemetry for
  fleet nodes.
- **S13VxFx** buck-boost + **Mini MOSFET Slide Switch (LV)** — the battery-node
  power front-end pair.
- **Tic 36v4** and **Motoron** I²C controllers, and the **Micro/Mini Maestro** —
  substrates for a future actuator cog.
