# Redrob Film vs Kdenlive: measured gap (priority tasks)

Kdenlive 26.08.2 (official AppImage) against Redrob Film 0.3.0 (release) and the 0.4.0 build,
on the same host (32 cores, llvmpipe, no GPU, PulseAudio null sink), with the same scripts and
media. Task definitions: [kdenlive-tasks.toml](kdenlive-tasks.toml). Raw verdicts:
[results-kdenlive.toml](results-kdenlive.toml), [results-film.toml](results-film.toml) (0.3.0),
[results-film-new.toml](results-film-new.toml) (0.4.0 build). Feature parity lives in
[../parity](../parity) (`kdenlive-*.toml`).

| Task | Film 0.3.0 | Film 0.4.0 | Kdenlive |
|---|---|---|---|
| Idle CPU, 1080p sequence stopped | 638–661 % | 0–18 % | – |
| SC-01 needle drag p50 / p95 | 68 / 104 ms | 63 / 82 ms | 53 / 66 ms |
| SC-01 monitor shows the exact frame | 61 / 80 ms | 62 / 146 ms (warm 61–63 / 67–70) | 121 / 151 ms |
| SC-02 frames shown while dragging | 40–45 /s | 40–47 /s | 13–16 /s |
| PB-01 first picture after Space (sink suspended) | 865–894 ms | 197 ms (keep-alive) | 60–87 ms |
| PB-02 1080p30, 20 s | 317/600 frames, 45 % dropped | 597/600, 0 dropped | 600/600, 0 dropped |
| AU-01 level / pitch | −33.06 dBFS, one 12 ms dropout | −33.05 dBFS, 0 dropouts | −33.05 dBFS |
| AU-02 silences ≥ 10 ms in 30 s | 2 | 0 | 0 |
| SC-04 scrub audio | 86–98 % of windows | 81–98 % | 100 % |

## Root causes found

- **Idle repaint (0.3.0):** on X11/Wayland winit reports an empty window title; the shell took it for
  someone else's title and re-sent its own every frame, and each viewport command asks egui for
  another frame. The window never went idle, and the software rasteriser kept six cores busy,
  which is what dropped playback frames and delayed the needle. Fixed in `shell.rs`
  `sync_window_title`.
- **First play delay:** the sound server suspends an idle output; Film opened its stream only on
  Space and waited for the device to wake. Kdenlive keeps a stream open. Fixed with a silent
  keep-alive stream (Linux, Windows) in `apps/redrob-film-app/src/audio.rs`.
- **Remaining:** the needle trails Kdenlive by ~10 ms p50 (software rendering of the full window
  each drag step); scrub audio cuts out ~0.5 s before the end of a long drag.
