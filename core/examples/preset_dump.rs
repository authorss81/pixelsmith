//! Print the JSON the pipeline editor's round-trip test compares against.
//!
//! `presets::to_pipeline` is pure arithmetic over a `Preset`, and it is what the
//! engine will actually run for a preset — but there is no FFI entry point that
//! returns it, because `px_presets` returns the catalogue and the *request* that
//! runs is built by the caller. So the expected half of
//! `app/test/preset_round_trip_test.dart` has to be generated from here rather
//! than written out by hand in Dart, or the test would only be checking that the
//! Dart side agrees with somebody's reading of `to_pipeline`.
//!
//! Run it after changing anything in `presets.rs`:
//!
//! ```text
//! cd core && cargo run --example preset_dump
//! ```
//!
//! The output is what `app/test/fixtures/preset_pipelines.json` holds. It is an
//! example and not a test because nothing in this repository asserts against it
//! directly — `app/` is a submodule this pipeline can read and cannot push to,
//! so a Rust test that failed when the app's copy drifted would be red with no
//! fix available. The Dart test is the assertion, and it runs where the fixture
//! lives.

use pixelsmith_core::presets::{PRESETS, to_pipeline};

fn main() {
    println!("[");
    for (i, preset) in PRESETS.iter().enumerate() {
        let (pipeline, format, quality, max_bytes) = to_pipeline(preset);
        // The whole preset, not just what `to_pipeline` derived from it: the
        // Dart test rebuilds a `Preset` from this row and feeds it back through
        // the editor, so the row has to carry the inputs as well as the
        // expected outputs or the test would be checking a hand-built fixture
        // against itself.
        let row = serde_json::json!({
            "id": preset.id,
            "label": preset.label,
            "category": preset.category,
            "width": preset.width,
            "height": preset.height,
            "fit": preset.fit,
            "filter": preset.filter,
            "format": preset.format,
            "quality": preset.quality,
            "max_bytes": preset.max_bytes,
            "crops": preset.crops,
            "chroma": preset.chroma,
            // What the editor must end up producing.
            "to_pipeline": {
                "pipeline": pipeline,
                "format": format,
                "quality": quality,
                "max_bytes": max_bytes,
            },
        });
        // Hand-formatted rather than `to_string_pretty`, so the committed file
        // keeps its key order and its one-row-per-preset shape: a diff of this
        // file has to read as "this preset changed", not as "serde_json moved a
        // comma".
        let body = serde_json::to_string(&row).expect("a preset row is serialisable");
        println!("  {}{}", body, if i + 1 == PRESETS.len() { "" } else { "," });
    }
    println!("]");
}