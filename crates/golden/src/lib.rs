//! Golden-image tests for Redrob Film's renderer (test-only crate; the library is empty).
//!
//! `tests/golden.rs` builds small procedural projects (no media files), renders one frame of each
//! through the CPU reference compositor (`redrob-film-render`) and compares it with a committed PNG
//! in `goldens/` using `redrob_film_testkit::golden`. The same scenes are composited on the GPU
//! (`redrob-film-gpu`) when an adapter is available and compared with the CPU result.
//!
//! Regenerate references with `REDROB_FILM_BLESS=1 cargo test -p redrob-film-golden`.
