# opencc600 — product definition

What the firmware is for, the rules it follows, where it stands. Hardware and reverse engineering:
[REVERSE_NOTES.md](REVERSE_NOTES.md); how we build and test: [DEV_PROCESS.md](DEV_PROCESS.md); the crates:
[DEVELOPMENT.md](DEVELOPMENT.md#code-layout).

## Principles
- **Rust, `no_std`**, on [`esp-hal`](https://github.com/esp-rs/esp-hal) + Embassy; no ESP-IDF, no LVGL. Everything
  but `firmware/` compiles on a PC, which is what makes most of the testing automatic.
- **c606-oss is a hardware reference, not the application**: its drivers (ST7789 i80 panel, nRF link, GNSS
  commands, FIT writer, UTC) were rewritten in Rust with its `docs/HARDWARE.md` at hand; its UI is not what we
  want. Not ported: eMMC, touch, LVGL, Mapsforge maps.
- **Radio in Rust**: BLE with `esp-radio` + `trouble-host`, Wi-Fi later with `esp-radio` + `embassy-net`. ANT+
  stays on the nRF, driven over the UART link.
- **Tested as automatically as possible, on the real device too** (HIL tests over USB).
- **No action without feedback**: every key press gets a visible answer at once (the screen changes, a banner, or
  why nothing happens). Anything that takes longer than a moment (saving a ride, a search, a transfer, a power
  off) says what is going on while it runs, then how it ended. Keys that do nothing meanwhile are hidden and their
  presses dropped. Applies to every new screen; checked in review.
- **License**: GPL-3.0-or-later. The formats and the protocol shared with the app are MIT, in
  [musette](https://github.com/enavarro222/musette).

## Product
1. **Use**: outdoor rides, **road and MTB**, for the author or anyone with a CC600. Home trainer use later, if at all.
2. **What the vendor firmware gets wrong**: no map around the planned route; heavy menus, too many useless functions.
3. **Core that must never break**: reliable FIT recording (crash, empty battery), the display, the sensors, a D+
   at least as good as the vendor's (barometer, [ALTITUDE.md](ALTITUDE.md)).
4. **What makes it different**: a corridor map around the loaded route, a lean 3-key UX, easy ride sync.
5. **Configuration and sync** in the companion app, [Musette](https://github.com/enavarro222/musette), over BLE;
   later over Wi-Fi back home, to a configurable server. The device menu stays minimal.
6. Later ideas: long-stop detection, configurable screens, BLE remotes, structured workouts (interval sessions
   programmed in the app, followed on the device), FTMS home trainer control, home_trainer_game.

## Hardware constraints
![CC600 hardware architecture](hardware.svg)

- 240x320 colour panel, **3 keys, no touch**: the whole UX is button driven.
- ESP32-S3, 2 MB quad PSRAM, app ≤ **0x420000 (4.1 MB)** per OTA slot; our image in `ota_0`, the vendor's in `ota_1`.
- The **nRF co-processor** (vendor firmware, never replaced) owns the keys, power, charger, RTC, barometer, light
  sensor, buzzer and ANT+, reached only over UART2 ([NRF_FIRMWARE.md](NRF_FIRMWARE.md)). Its sensor stream
  (barometer, light, RTC) only starts on a long press of the power key: after a power cut without it the firmware
  shows the charging screen, as the vendor does.
- The barometer's pressure reaches the ESP32 already filtered by the nRF (~2 s late).
- The nRF RTC is set from the GNSS time on a valid fix (the time before the first fix may be wrong).
- No IMU on the CC600 (the C606's nRF streams one, see REVERSE_NOTES.md).
- 1800 mAh battery: battery life is a requirement ([POWER.md](POWER.md)).

### Storage
- The whole 7.8 MB littlefs `storage` partition is ours, after a verified backup (`dump/<mac>/storage.orig.bin`) and the
  vendor's 24 rides extracted as FIT (`tools/reverse/vendor_fit.py`). Back to the vendor:
  `tools/flash.py --restore-vendor --storage dump/<mac>/storage.orig.bin` (checked on the device).
- Rides: `/rides/<start>.part` while recording, `.fit` once closed, `.sent.fit` once Musette acknowledged it
  (resumed, it is `.fit` again). Power-cut tested at every flash operation (`crates/ride-store`).
- **Rides first** (`ride_store::room`): the logs (`/logs/*.csv`) take 2 MB at most and never the last 1 MB. Below
  1 MB left, a ride records a point every 5 s instead of every second (~40 h more; the floppy disk of the top bar
  turns red); a ride does not start below 64 KB: the "Mémoire pleine" screen lists the rides to delete one at a
  time, the ones the app has first. **The device never deletes a ride on its own.**
- Wear: an append every 10 s copies its last block (~360 erases/h, ~750 with the trip log), spread by littlefs over
  the free blocks; the reserve keeps the storage from staying nearly full.

## UI rules
Clickable prototype: https://enavarro222.github.io/opencc600/prototype/ (source in [prototype/](prototype/)),
kept in step with `crates/ui`.
- Every screen shows what each of the 3 keys does, next to the key: bottom-right = main action (start, pause,
  resume, OK), bottom-left = menu / next, top-left = back / contextual. The 3 labels have the same size and a blue
  mark (the colour of the physical keys) on the key's side.
- In a menu, the select key names the action of the entry ("Reprendre", "Éteindre", "Ouvrir ›"); a chevron marks
  the entries that open a screen.
- No hidden action. Two long presses only, both labelled: "⏻ Maintenir" on the home screen to switch off (and on),
  and the **key lock** (to do): both bottom keys held together (~1 s) lock the keys during a ride and unlock them;
  a padlock in the top bar, any press shows "Verrouillé · maintiens les 2 boutons du bas", alerts and pages go on,
  not kept across a power off.
- Irreversible actions go through a confirmation that says what will happen, confirmed with another key than the
  one that opened it. Abandoning a ride takes two, the keys swapped on the second (two presses out of habit erase
  nothing). A ride started by mistake (nothing recorded) is offered for erasing instead of saving.
- Riding: up to 4 pages, picked automatically (below); bottom-left cycles them, top-left is the map's zoom on the
  map, auto-scroll on / off elsewhere.
- Top bar: the top key on the left in a fixed 100 px box, the clock in the middle ("GPS ✕" in its place while
  riding without a fix, the distance to the route when off it, two figures: "85m", "9,9km", "12km"), the Bluetooth
  rune while the phone's app is connected, the floppy disk while recording and the battery on the right. The clock
  never moves (`top-bars.png` of the screens test shows every case).
- Nothing changes size: fixed-width top key, key bar in 3 fixed columns, fixed-height rows; long text is cut with
  "…" (`tools/check_prototype_layout.py`, and the screens test of `crates/ui`).
- One banner at a time, floating above the page, never over the key bar: information (blue, 3 s), attention
  (amber, while true: GPS searching or lost, off route, storage full, automatic power off coming).
- GPS: the home screen shows the acquisition; a ride can start before the fix (the distance starts at the fix).
- Pause: bottom-right resume, bottom-left "Terminer…" (confirmed), top "≡ Menu" (sensors, route, switch off,
  status, abandon). Switched off while paused, the ride resumes after power-on.
- A finished ride stays **resumable until the app has it** (end screen, then the menu): finishing only closes the
  FIT file, resuming reopens the same recording. Once sent it cannot be resumed (it would duplicate the activity).
- **One route on the device**, sent from the app at any time. **Free ride**: "Rouler sans trace" / "Suivre la
  trace" in the menu and the pause menu, even mid-ride; the route pages and alerts go away, the route stays. The
  home screen always says which ("Parcours libre" or the route). Started with the wrong route by mistake: the
  off-route banner says "Pas cette trace ? Pause › Menu"; the device never drops the route on its own.

## Ride pages: chosen for the rider, not built by the rider
No page editor. The app asks **one question** ("explore" or "train"); the device sees the rest when a ride starts
(route loaded, power meter or heart rate sensor connected) and picks the pages (`ui::pages`):

| Usage | With route | Without route |
|---|---|---|
| Explore | Map · Essential · Climbs or Altitude (· Effort) | Essential · Altitude (· Effort) |
| Train | (Effort ·) Essential · Map · Climbs or Altitude | (Effort ·) Essential · Altitude |

Effort is the power page with a power meter connected, else the Cardio page with a heart rate sensor connected
(`ui::EffortPick`): a power meter off at the start leaves Cardio. In the first 5 minutes of a ride (or after a
sensor is paired), a sensor that connects brings its page, Effort in place of Cardio; later, a sensor lost keeps
its page with "–".

- **Essential**: speed (big) on a gauge from 0 to 50 km/h with a tick at the average, average and max speeds in
  the corners, time, distance, D+, heart rate with its zone if paired (else cadence, else the time since the
  start).
- **Altitude**: altitude, gradient (last 40 m), D+, climbing speed (VAM), the last 5 km.
- **Effort** (a power meter paired): 3 s power (big), average power, heart rate, cadence, speed, time. The top
  takes the colour of the power zone: its number and range, % FTP, a ruler of the zones.
- **Cardio** (a heart rate sensor, no power meter): heart rate (big) in the colour of its zone, % of the max heart
  rate, then average and max heart rate, speed, cadence or time.
- **Zones**: `ftp` and `hr_max` (0 by default: not set, no zones, the page says to set them in the app), and the
  bounds of the zones in % of them, `power_zones` (Coggan's 7 by default) and `hr_zones` (5 by default); 2 to 7
  zones, coloured grey to purple. A zone shows once the value has stayed 3 s in it (no flicker at a bound). No FTP
  or max heart rate is guessed: a wrong one gives wrong zones.
- **Map** (a route): turned to the heading, the part done in grey, remaining distance and speed; zooms with a
  scale bar of 250 m to 2 km, then the whole route.
- **Climbs** (a route with climbs, in place of Altitude): altitude, gradient and D+ on top, then the last climb
  done, the next one with its distance, the following ones. In a climb: its profile by 1 km bands coloured by
  gradient with the rider on it, distance, D+ and average gradient left, gradient, VAM, time to the top. The
  climbs are found by Musette when it builds the route (crate `climbs`) and travel in the route file.
- Distances under 1 km in metres ("850 m"), then km with one decimal.
- A sensor missing at start takes no slot; a sensor lost mid-ride keeps its slot with "--" plus a banner: the
  layout never moves during a ride.

## Features and settings
| Feature | Status | Default |
|---|---|---|
| Off-route alert (banner, distance in the top bar) | done | on |
| Off-route beep, dashed line back to the route | later | beep configurable |
| Climbs (list, live climb page) | done | on |
| Climb ahead alert | later | on |
| Page auto-scroll | done, toggled on the device | interval in the app |
| Automatic power off when still during a ride | done | 20 min, 0 = never |
| Day / night theme | done (manual) | day |
| Night theme from sunset to sunrise (from the GPS position) | done | on |
| Backlight from the ambient light sensor | done, curve to tune on the road | on |
| Auto-pause (timer stops when not moving) | later | on |
| Start reminder ("tu roules, démarrer ?"), never starts on its own | later | on |
| Turn announcements (cues precomputed by the app) | later | off |
| Laps | later | off |
| Strava segments (needs a Live Segments licence) | maybe | off |

## Corridor map around the route
No global map, but a map restricted to the loaded route fits the CC600 (prototype on 2026-10-20, not in the
firmware yet):
- **One route at a time**, so the storage stays bounded.
- **Prepared off the device**: OSM clipped to a ±1 km corridor and projected by the app, sent over BLE with the
  route (5–20 KB/s, so a few hundred KB at most); Wi-Fi to download it later. The device only draws polylines and
  areas.
- **Two views, both essential**: the whole ride on its background, and the few km around the rider. Two levels of
  detail cut in cells (500 m for the detail), the device reads only the cells in view.
- Measured: 0.5 KB/km² in the country, 2.3 in forests, 10 in a town centre: 100–400 KB for 100 km outside big
  towns (42 km route: 67 KB, overview 13 KB). Contour lines every 10 m (EU-DEM) add ~75 KB. Drawn in 47–107 ms on
  the device.

## MVP
Done:
- FIT recording that survives a crash or an empty battery; interrupted rides closed and offered again.
- GPS time written back to the nRF RTC; time zone as a POSIX rule with summer time.
- Barometric altitude and D+ ([ALTITUDE.md](ALTITUDE.md)).
- The screens and keys of `crates/ui`, after the prototype.
- Routes from Musette (BLE) or USB, the map page, the climbs, the free ride.
- Settings in flash (`settings-cc600`), changed from Musette.
- BLE sensors (heart rate, speed / cadence, power and its calibration).
- Rides listed, sent to Musette and acknowledged.
- Automatic power off during a ride; a full storage handled.

To do:
- **Beeps** for the alerts (off route, sensor lost, low battery) and key feedback, each with a message on screen;
  ride start, pause, resume and save already beep.
- **GPS fix as early as possible**: hot start with the last position and the RTC time sent to the receiver
  (`$PAIR` aiding); later A-GNSS data from the app or Wi-Fi. Measured against the vendor with `tools/ttff.py`.
- **Key lock** (see the UI rules).
- **Battery life** ([POWER.md](POWER.md)): battery-run log, clean power off at low battery, a reference run
  against the vendor firmware, then the levers.
- **ANT+** with real sensors (written and tested on the PC, on the device only without a sensor).
- The open questions below: each one explained, or its impact known and accepted.

After the MVP: the corridor map, then Wi-Fi sync.

## Open questions
Each one: what we know, why it matters, how to find out. Details in [REVERSE_NOTES.md](REVERSE_NOTES.md).

1. **Airoha outputs not turned off by `$PAIR062`**: `$PAIRSAT` (4/s, ~1.2 KB/s, most of the remaining GNSS
   traffic), `GNACCURACY`, `GNEPE`, `PLSV`, `PAIRDGP`. Matters for power. How: the Airoha PAIR command guide
   (RYS352x / AG3335 manuals), the `$PAIR` strings the vendor sends (`866`, `867`, `490`, `491`, `470`, `382`,
   `590`, `020`, `023`: try each, compare the output).
2. **USB serial link loses lines** (~5–7 % of the commands and log lines) when the log is busy. Worked around for
   the tests (numbered commands, acks, resends), root cause unknown. Matters for the HIL tests and any USB sync.
   How: a minimal firmware that only echoes and logs, esp-println's USB-Serial/JTAG writer vs the esp-hal driver,
   then an upstream issue.
3. **Ambient light tags 2000 / 3000**: tag 600 is the visible light (411 covered, ~2600 under a torch:
   saturates?), 2000 and 3000 stay at 0–6. Matters for the automatic backlight. How: an infrared remote, direct
   sunlight, darkness; the sensor part number.
4. **nRF stream `F1 0A`** (3.3 Hz, tracks the barometer temperature): meaning unknown. How: heat / cool the device,
   compare with the barometer and battery temperatures.
5. **A key click seen twice?** A pause and a resume once arrived in the same second. Double press, or the nRF
   sending `Click` twice? A bounce would pause the ride. How: log the raw key frames during single clicks.
6. **GNSS time without a fix ~22 s off**: leap seconds (18 s) not applied, plus drift? Would tell when the
   receiver's time can set the RTC before a fix. How: watch the offset from a cold start to the first fix.
7. **Is the 5 s power-on command to the nRF needed?** The vendor and c606-oss repeat it. Matters for power and log
   noise. How: stop sending it for an hour and watch the nRF.
8. **Panel readable in sunlight with the backlight off?** Sets the backlight strategy. How: outside, backlight at 0.

## Out of scope
- Global maps (Mapsforge style), on-device routing and turn-by-turn recalculation.
- An OTA update path is needed once other people install the firmware (with a safe fallback): not before.
