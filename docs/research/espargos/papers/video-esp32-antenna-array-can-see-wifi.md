# Video: "This ESP32 Antenna Array Can See WiFi"

https://www.youtube.com/watch?v=sXwDrcd1t-E . Channel Jeija (the author's own channel) [V, YouTube oEmbed]. Uploaded 2025-02-14, duration 643 s (10 min 43 s), no chapters [V, yt-dlp metadata].

Sources used: description and English auto-generated captions (yt-dlp). I did not watch the video. Captions are ASR with errors ("Lano side" for line-of-sight, "esp32s" spelling drift); statements marked [V, ASR].

## Description content [V, video description]

Links the project site https://espargos.net/, the pyespargos repo (`demos/camera` for the "WiFi camera"), dataset espargos-0005 for the AoA / TDoA visualisation, and three code repositories by the author: `Jeija/ToA-AoA-Augmented-ChannelCharting`, `Jeija/Geodesic-Uncertainty-Loss-ChannelCharting` (source of the channel-charting animation), and the DICHASUS channel-charting tutorial (https://dichasus.inue.uni-stuttgart.de/tutorials/tutorial/dissimilarity-metric-channelcharting/). The author states the origin: colleague Marc Gauger proposed ESP32 chips instead of SDRs; students Tim Schneider, David Engelbrecht and David Kellner helped build it. Funded by BMBF within Open6GHub (grant 16KISK019). ARENA2036 hosted the channel-charting measurement campaign. I did not open the two extra repos, so their licences are not reviewed.

## Content of the video [V, ASR]

- Demos (about the first 5 min): the array overlays a phone's WiFi traffic on the camera image; a metal wall behaves like a mirror, and the reflected path has a yellowish tint because of higher delay; blocking the direct path leaves only the reflection; non-metallic walls only attenuate, so a device in the next room stays visible; the software filters packets by MAC address so only one device is shown; a Yagi antenna behaves like a flashlight; passive targets: tin foil gives a bright spot and non-reflective targets appear as shadows in front of existing paths, so "the array can act as a passive radar system by exploiting existing Wi-Fi signals like SSID broadcasts from an access point". Outdoors, "with 40 MHz of bandwidth in the 2.4 GHz Wi-Fi spectrum we can achieve some level of depth perception", with path delay shown as colour.
- Price claim: the captions say a chip "that you can get for less than [euro]150" but the auto-caption is garbled, and it is unclear whether this refers to the ESP32 chip or to the array. Treat as not established.
- Sync explanation (as in the SDR Academy talk): 40 MHz shared reference gives frequency sync; the PLL adds a random phase offset after every power-up, described as "phase uncertainty"; reference packets travelling on microstrip lines of known length are measured at each receiver to compensate. "This calibration procedure is only necessary once after each ESP32 has booted up or in case we switch the Wi-Fi channel."
- CSI: a phase and amplitude per subcarrier; an FFT over subcarriers gives the channel impulse response.
- Localisation: with several arrays, angle-of-arrival triangulation; with shared clock and phase reference, TDoA too; the best accuracy combines both. Combined arrays with phase-balanced splitters give more accurate AoA and a higher image resolution.
- NLoS: triangulation and TDoA trilateration assume LoS. For a single array behind obstacles, self-supervised channel charting on a dataset collected by a moving transmitter can learn the geometry. The resulting chart is "fairly accurate in both line-of-sight and non-line-of-sight areas", and "real-time" NN localisation is possible [V, ASR].

## Use for this pool

The video adds the passive-radar behaviour (foil = bright spot, absorber = shadow) that the papers formalise as "passive channel charting", and the statement that the phase calibration is needed once per boot or channel switch. It adds no quantitative results.
