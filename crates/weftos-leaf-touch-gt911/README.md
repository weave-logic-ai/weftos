# weftos-leaf-touch-gt911

GT911 capacitive-touch driver + scene-aware hit-test layer for WeftOS
leaf devices (Phase E).

`no_std + alloc`. Generic over `embedded_hal_async::i2c::I2c`.

## What it gives you

- **`Gt911::new(i2c)`** — probes both factory addresses (0x14, 0x5D)
  and binds to whichever responds.
- **`Gt911::read_frame()`** — low-level: returns the raw status byte +
  optional `TouchFrame` (up to 5 simultaneous points).
- **`Gt911::poll_events()`** — high-level: tracks per-finger `id`
  across polls, emits `Down`/`Move`/`Up` `TouchEvent`s automatically.
- **`hit_test_event(&store, display, event)`** — converts a raw
  `TouchEvent` into a wire-ready `weftos_leaf_scene::InputEnvelope`,
  with `node_id` resolved via `SceneStore::hit_test`.

## Typical wiring (Embassy + esp-hal)

```rust,ignore
let mut gt911 = Gt911::new(i2c).await?;

loop {
    let events = gt911.poll_events().await?;
    for ev in events {
        let env = hit_test_event(&*store.lock(), 0, ev);
        let cbor = weftos_leaf_scene::codec::encode(&env)?;
        publish_on("mesh.leaf.<pk>.input", &cbor).await;
    }
    Timer::after(Duration::from_millis(20)).await;
}
```

## Hardware notes (CrowPanel DIS08070H)

- GT911 RST is on **PCA9557 IO1**, not a GPIO. The PCA9557 reset dance
  MUST run before opening an I²C session. See
  `clawft-edge-pad::drivers::pca9557::reset_board_peripherals`.
- The factory config blob is `version=0xFF` valid — do NOT rewrite.
- The chip answers on 0x14 first (factory v3.0+), 0x5D as fallback.
