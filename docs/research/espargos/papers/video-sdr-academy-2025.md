# Video: SDR Academy 2025, "Making WiFi Visible with ESPARGOS and ESP32s"

https://www.youtube.com/watch?v=GrlRUA7dW44 . Title: "SDRA'25 - 03 - Florian Euchner, DO7JE: Making WiFi Visible with ESPARGOS and ESP32s", channel Software Defined Radio Academy [V, YouTube oEmbed]. Uploaded 2025-08-22, duration 1806 s (30 min 6 s) [V, yt-dlp metadata]. Held at the SDR Academy alongside the HAM radio conference in Friedrichshafen [V, espargos.net/research].

Sources used: the video description, chapter list and the English auto-generated captions (via yt-dlp). I did not watch the video, so anything visual only (slides, screens) is not reviewed. Captions are ASR and contain errors, for example "face" for "phase". Numbers below are read from captions and marked [V, ASR].

Chapters [V, yt-dlp]: Introduction and demo 0:00; System overview 1:44; Wi-Fi signals and channel state 6:30; Hardware and synchronization 12:30; Software and data processing 16:22; Q&A and closing 23:50.

## Content

- System: an 8x4 antenna array with an ESP32 behind each antenna, "32 ESP32s in this picture", a webcam on top, a computer doing CPU and GPU processing, and a screen [V, ASR].
- Demos: a phone generating traffic becomes visible in an overlay; a metal wall acts as a mirror (reflection visible only at the right geometry, angle of incidence equals angle of reflection); gypsum "triwall" is "more transparent to microwaves than glass"; a Yagi antenna lights up when pointed at the array or at a reflector; a transmitter inside a microwave oven can still be tracked because the oven gives "around 30 dB of attenuation"; a Faraday bag from a vendor "offgrid" does attenuate enough, but a cable poking out of it acts as an antenna and leaks the signal [V, ASR].
- Signal basics: the L-LTF pilot in the PPDU, OFDM subcarriers, Y(f) = H(f) X(f), and the ESP32 computes H(f) in hardware "so it's not even a software defined radio but a hardware radio" [V, ASR].
- Architecture framing: analog phased array vs fully digital array vs ESPARGOS, which is "a mixture": a complete receive chain with some DSP on each chip and some on a central computer. Cheap, but less array gain for demodulation than a fully digital array [V, ASR].
- Synchronisation: shared 40 MHz clock gives frequency sync; the PLL gives an unknown LO phase after reset ("phase uncertainty"); a reference packet over microstrip lines of known length lets software subtract the phase error. "The synchronization then happens in software" [V, ASR].
- Data cube: one packet is an (8 x 4 antennas) x subcarriers array. At 40 MHz, 117 active subcarriers [V, ASR; matches arXiv 2408.16377 Fig. 3].
- Processing: 2-D FFT over the antenna axes gives beamspace; zero-padding oversamples it; a diagram maps beamspace to azimuth and elevation; pinhole camera model overlays it. An FFT over frequency gives delay (colour: blue short, red long). A further FFT over packets could give relative velocity (not shown). Localisation: four arrays give triangulation, and if time-synchronised, TDoA and multilateration, or a combination [V, ASR].
- Roadmap: a small-scale manufacturing run is being attempted, no date promised [V, ASR].

## Q&A (as far as captions allow)

- Arbitrary traffic: works if the device is on the correct WiFi channel and generating traffic. Multiple devices possible (author will demonstrate at the booth) [V, ASR].
- CSI documentation: ESP32 CSI "is very well documented", the author says other vendors (he names Infineon) refused access; what is undocumented is how to phase-synchronise them [V, ASR].
- Direct path vs reflections: this is the standard indoor challenge. High bandwidth helps; Bluetooth systems struggle because of narrow bandwidth; at 40 MHz WiFi you can tell first arrival from reflection in a large enough room [V, ASR].
- Firmware is in C, the processing library is Python [V, ASR].
- Quality: ESP32 CSI is 8-bit signed for real and imaginary part, limiting dynamic range. CSI quality depends on synchronisation, which depends on reference-signal amplitude and packet modulation. "It's not great with the ESP32. If you used an expensive SDR you get much higher quality CSI", but more antennas compensate somewhat. The camera image is close to diffraction limited [V, ASR].

## Use for this pool

This talk is the best short source for the sync architecture and the quality caveats in the author's own words. It contains no numeric accuracy figures. Its mention of 8-bit CSI and dependence on reference amplitude is a limitation that the papers do not state as directly.
