//! The JSON contract, as data.
//!
//! `ffi_abi.rs` does this for the C ABI: the declarations are written once, as
//! data, and checked against `ffi.rs` at compile time. This module is the same
//! idea for the JSON, which is where the drift actually happened.
//!
//! ## Why this exists
//!
//! `scripts/check-dart-bindings.sh` proves the app's `bindings.dart` matches the
//! engine's C ABI, and it reports "15 entry points match" while five phases of
//! JSON fields are missing from the Dart models. `docs/AUDIT.md` findings 3, 4
//! and 5 are all the same defect: the engine's contract grew and the Dart side
//! was never told, because nothing compared the two.
//!
//! A source-level diff would be the obvious answer and the wrong one. It would
//! have to re-implement `#[serde(rename_all = ...)]`, `#[serde(tag = ...)]`,
//! `#[serde(flatten)]` and `Option` handling to produce the *wire* names, and
//! every one of those is a place a hand-rolled parser and serde disagree — which
//! is a check that reports drift that is not there and misses drift that is.
//!
//! So the field names here are **asked of serde**. Every line is produced by
//! serialising a representative instance of a real contract type and reading the
//! keys out of the resulting `serde_json::Value`. If a field is renamed, made
//! conditional, or moved, this module's output changes, because serde's output
//! changed — not because a rule here was updated to agree with it.
//!
//! ## What the caller does with it
//!
//! `scripts/check-json-contract.sh` diffs this list against
//! `core/contract/json-fields.txt`, which is a committed file in *this*
//! repository naming the Dart symbol that carries each field. A field the engine
//! emits with no committed row is a failure; a committed row naming a Dart symbol
//! that nothing declares is a failure. Both directions matter: the first is how
//! drift arrives, and the second is how a row rots into a lie.
//!
//! The committed side being here rather than in `app/` is the whole trick. The
//! app submodule is a separate repository this pipeline can read and cannot push
//! to, so a check whose answer depends on what the app repository contains is a
//! check that cannot fail — see `docs/AUDIT.md` finding 17 and the note in
//! `scripts/verify.sh` section 3b about a gate check that had to be downgraded to
//! a note for exactly that reason. A committed list here is landable, so the
//! check can be a hard gate even while the fix cannot.

use crate::animation::{AnimationAction, AnimationOutcome, AnimationPolicy};
use crate::colour::{ColourOptions, ColourOutcome, ColourProfile, ColourSpace};
use crate::format::{ChromaSubsampling, OutputFormat};
use crate::pipeline::{CropSpec, FitMode, Orientation, Pipeline, ResampleFilter, ResizeSpec};
use crate::presets::Preset;
use crate::validate::ValidateReport;
use crate::worker::{BatchPolicy, BatchReport, Outcome, SizeUnit, SkipReason};

/// One line of the contract: a dotted path through the JSON.
///
/// A struct field is `owner.field`; a field of a nested struct is
/// `owner.outer.inner`, because that is what a caller has to type to reach it.
/// An enum variant is `owner::variant`, because a variant is a value rather
/// than a key and conflating the two would make the two checks ambiguous.
pub type ContractLine = String;

/// Every field and every variant in the JSON that crosses the FFI, sorted.
///
/// Sorted, so the committed file is a diff rather than a re-order, and so
/// "added a field" shows up as one added line rather than a reshuffled file.
pub fn contract() -> Vec<ContractLine> {
    let mut lines: Vec<ContractLine> = Vec::new();

    // ---- response envelopes -------------------------------------------
    push_struct(&mut lines, "ValidateReport", &sample_validate_report());
    push_struct(&mut lines, "ExifInfo", &sample_exif_info());
    push_struct(&mut lines, "ExifEntry", &sample_exif_entry());
    push_struct(
        &mut lines,
        "ProcessResponse",
        &crate::ffi::sample_process_response(),
    );
    push_struct(&mut lines, "Outcome", &sample_outcome());
    push_struct(&mut lines, "BatchReport", &sample_batch_report());
    push_struct(&mut lines, "Capabilities", &crate::capabilities());
    push_struct(&mut lines, "Preset", &sample_preset());
    push_struct(
        &mut lines,
        "StructLayout",
        &crate::ffi_abi::px_buffer_layout(),
    );
    push_struct(&mut lines, "FieldLayout", &sample_field_layout());

    // ---- request envelopes --------------------------------------------
    // Read by the app, so a field the engine honours and the app cannot send is
    // drift in the same direction as a response field it cannot read. Five
    // phases of missing Dart models are five phases of this going unnoticed.
    push_struct(
        &mut lines,
        "ProcessRequest",
        &crate::ffi::sample_process_request(),
    );
    push_struct(
        &mut lines,
        "BatchRequest",
        &crate::ffi::sample_batch_request(),
    );
    push_struct(&mut lines, "BatchFile", &crate::ffi::sample_batch_file());

    // ---- nested value types, named by where they appear ----------------
    push_struct(&mut lines, "ColourProfile", &ColourProfile::default());
    push_struct(&mut lines, "ColourOutcome", &ColourOutcome::default());
    push_struct(&mut lines, "ColourOptions", &ColourOptions::default());
    push_struct(&mut lines, "AnimationOutcome", &AnimationOutcome::unknown());
    push_struct(&mut lines, "Pipeline", &Pipeline::new());
    push_struct(&mut lines, "CropSpec", &sample_crop_spec());
    push_struct(&mut lines, "ResizeSpec", &ResizeSpec::default());
    push_struct(&mut lines, "BatchPolicy", &BatchPolicy::default());

    // Every `SkipReason` variant, because an internally tagged enum's fields
    // differ per variant: `Duplicate` has `of` and `WouldUpscale` has two pairs
    // of dimensions. Sampling one variant and calling that the enum's fields
    // would miss four of them.
    push_skip_reasons(&mut lines);

    // ---- enum variants -------------------------------------------------
    // Every variant, because the wire names come from `rename_all` and not from
    // the Rust names: `DisplayP3` is `display_p3` and `Luma444` is `luma444`,
    // and an enum the Dart side spells differently is a refusal at the boundary.
    push_variants(&mut lines, "OutputFormat", OutputFormat::all());
    push_variants(
        &mut lines,
        "ChromaSubsampling",
        ChromaSubsampling::ALL.as_ref(),
    );
    push_variants(
        &mut lines,
        "AnimationPolicy",
        &[AnimationPolicy::Keep, AnimationPolicy::FirstFrame],
    );
    push_variants(
        &mut lines,
        "AnimationAction",
        &[
            AnimationAction::Unknown,
            AnimationAction::Still,
            AnimationAction::Preserved,
            AnimationAction::Flattened,
            AnimationAction::Refused,
        ],
    );
    push_variants(
        &mut lines,
        "ColourSpace",
        &[
            ColourSpace::Untagged,
            ColourSpace::Srgb,
            ColourSpace::DisplayP3,
            ColourSpace::Other,
        ],
    );
    push_variants(&mut lines, "SizeUnit", &[SizeUnit::Bytes, SizeUnit::Pixels]);
    push_variants(&mut lines, "SkipReason", &sample_skip_reasons());
    push_variants(
        &mut lines,
        "FitMode",
        &[
            FitMode::Contain,
            FitMode::Cover,
            FitMode::Fill,
            FitMode::Width,
            FitMode::Height,
        ],
    );
    push_variants(
        &mut lines,
        "ResampleFilter",
        &[
            ResampleFilter::Lanczos3,
            ResampleFilter::CatmullRom,
            ResampleFilter::Triangle,
            ResampleFilter::Nearest,
            ResampleFilter::Box,
        ],
    );
    push_variants(
        &mut lines,
        "Orientation",
        &[
            Orientation::Normal,
            Orientation::MirrorHorizontal,
            Orientation::Rotate180,
            Orientation::MirrorVertical,
            Orientation::MirrorHorizontalRotate270,
            Orientation::Rotate90,
            Orientation::MirrorHorizontalRotate90,
            Orientation::Rotate270,
        ],
    );
    push_variants(&mut lines, "Category", &sample_categories());

    lines.sort();
    lines.dedup();
    lines
}

// ---------------------------------------------------------------------------
// Asking serde
// ---------------------------------------------------------------------------

/// Serialise `value` and record every key path it produces under `owner`.
///
/// Recursion is the point: serde emits nested objects as nested JSON, so one
/// call covers `ValidateReport.colour.icc_present` without this module knowing
/// that `ValidateReport` has a `colour` field of type `ColourProfile`. A
/// hand-written list of every field of every type would be a second copy of the
/// contract, which is the thing being checked for.
fn push_struct<T: serde::Serialize>(out: &mut Vec<ContractLine>, owner: &str, value: &T) {
    match serde_json::to_value(value) {
        Ok(value) => push_value(out, owner, value),
        // A `to_value` that fails would make this module silently report fewer
        // fields than the contract has, which is the one failure mode a drift
        // check must not have. It cannot happen for the types here — every one
        // derives `Serialize` and contains nothing exotic — so it is a panic
        // rather than a silent omission.
        Err(e) => panic!("{owner} failed to serialise: {e}"),
    }
}

/// Record the keys of an already-serialised object.
///
/// Separate from [`push_struct`] because the four FFI envelopes are private to
/// `ffi.rs`, so they arrive here as `serde_json::Value` rather than as a type
/// this module can name.
fn push_value(out: &mut Vec<ContractLine>, owner: &str, value: serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => walk_object(out, owner, &map),
        other => panic!("{owner} serialised as {other}, not an object"),
    }
}

/// Walk one JSON object's keys, recursing into nested objects.
///
/// An array of objects recurses into its first element without adding a segment,
/// so `ExifInfo.entries[0].tag` is recorded as `ExifInfo.entries.tag`: a list of
/// one shape has one set of keys, and the index is not something a caller types.
fn walk_object(
    out: &mut Vec<ContractLine>,
    prefix: &str,
    map: &serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in map {
        let path = format!("{prefix}.{key}");
        out.push(path.clone());
        match value {
            serde_json::Value::Object(nested) => walk_object(out, &path, nested),
            serde_json::Value::Array(items) => {
                if let Some(serde_json::Value::Object(first)) = items.first() {
                    walk_object(out, &path, first);
                }
            }
            _ => {}
        }
    }
}

/// Record the wire name of each variant of an enum.
///
/// Asked of serde the same way the fields are: serialise the variant and read
/// the string. An externally tagged unit variant is the bare string; an
/// internally tagged one (`SkipReason`, `#[serde(tag = "kind")]`) is an object
/// whose `kind` is the wire name. Both are read here rather than pattern-matched
/// on the Rust variant name, because the wire name is what the app sends.
fn push_variants<T: serde::Serialize>(out: &mut Vec<ContractLine>, owner: &str, variants: &[T]) {
    for variant in variants {
        match serde_json::to_value(variant) {
            Ok(serde_json::Value::String(name)) => {
                out.push(format!("{owner}::{name}"));
            }
            Ok(serde_json::Value::Object(map)) => match map.get("kind") {
                Some(serde_json::Value::String(name)) => {
                    out.push(format!("{owner}::{name}"));
                }
                _ => panic!("{owner} serialised as an object with no `kind`"),
            },
            Ok(other) => panic!("{owner} serialised a variant as {other}, not a name"),
            Err(e) => panic!("{owner} failed to serialise a variant: {e}"),
        }
    }
}

/// Every `SkipReason` variant, as a struct sample, so their per-variant fields
/// are recorded.
///
/// The enum is also covered by [`push_variants`] for its `kind` names; this is
/// the other half, because `Duplicate { of }` and `TooLarge { limit, actual,
/// unit }` have fields that exist on no other variant.
fn push_skip_reasons(out: &mut Vec<ContractLine>) {
    for reason in sample_skip_reasons() {
        // The variant name is the class of the value, which is exactly what
        // serde writes into the `kind` tag. `Debug` is the identity here because
        // it is only ever used to build a key for this module's own output — the
        // wire name comes from `push_variants`, which asks serde.
        let owner = format!(
            "SkipReason.{}",
            match &reason {
                SkipReason::Duplicate { .. } => "Duplicate",
                SkipReason::Unreadable => "Unreadable",
                SkipReason::UnsupportedFormat { .. } => "UnsupportedFormat",
                SkipReason::TooLarge { .. } => "TooLarge",
                SkipReason::WouldUpscale { .. } => "WouldUpscale",
            }
        );
        match serde_json::to_value(&reason) {
            Ok(serde_json::Value::Object(map)) => walk_object(out, &owner, &map),
            Ok(other) => panic!("{owner} serialised as {other}, not an object"),
            Err(e) => panic!("{owner} failed to serialise: {e}"),
        }
    }
}

/// One of every `SkipReason`, with every variant covered.
///
/// Hand-listed because an enum has no way to enumerate itself in Rust, and
/// because the *values* do not matter: only that each variant appears once so
/// its own fields and its wire name are both recorded. The trade is stated in
/// `docs/phase-status.md`: adding a `SkipReason` variant needs a line here, and
/// the check then fails on the committed contract file until it has a Dart model
/// — which is the behaviour this whole phase exists to produce.
fn sample_skip_reasons() -> Vec<SkipReason> {
    vec![
        SkipReason::Duplicate { of: String::new() },
        SkipReason::Unreadable,
        SkipReason::UnsupportedFormat {
            format: OutputFormat::Heic,
        },
        SkipReason::TooLarge {
            limit: 0,
            actual: 0,
            unit: SizeUnit::Bytes,
        },
        SkipReason::WouldUpscale {
            requested: (0, 0),
            actual: (0, 0),
        },
    ]
}

// ---------------------------------------------------------------------------
// Representative instances
// ---------------------------------------------------------------------------

// Representative instances
//
// Every value below exists only to be serialised and read back. They are built
// from the type's own constructors rather than by transcribing field names, so
// a field added to a struct is a **compile error here** rather than a silent
// omission from the contract — which is the failure mode a drift check must not
// have, and the reason these are functions with real arguments rather than
// `serde_json::json!` literals spelled out by hand.
//
// ---------------------------------------------------------------------------

fn sample_validate_report() -> ValidateReport {
    ValidateReport {
        format: OutputFormat::Jpeg,
        width: 1,
        height: 1,
        megapixels: 0.0,
        has_exif: false,
        has_animated: false,
        frames: 1,
        frames_truncated: false,
        sensitive_tags: Vec::new(),
        orientation: None,
        colour: ColourProfile::untagged(),
        suspicious: None,
    }
}

fn sample_exif_info() -> crate::exif::ExifInfo {
    // One entry, because the element shape is part of the contract and an empty
    // list would serialise to `[]` — which reports `ExifInfo.entries` and nothing
    // about what is inside it. Same reason the skip reasons are sampled per
    // variant.
    crate::exif::ExifInfo {
        entries: vec![sample_exif_entry()],
        colour: ColourProfile::untagged(),
        ..crate::exif::ExifInfo::default()
    }
}

fn sample_exif_entry() -> crate::exif::ExifEntry {
    crate::exif::ExifEntry {
        tag: String::new(),
        value: String::new(),
    }
}

fn sample_outcome() -> Outcome {
    Outcome::skipped(
        &crate::worker::Job {
            id: String::new(),
            name: String::new(),
            bytes: Vec::new(),
        },
        // `Unreadable` rather than a real skip: the sample is about the *field
        // names* of `Outcome`, and a skipped outcome reports them all, so no
        // `#[serde(skip_serializing_if)]` on any of them can hide one.
        SkipReason::Unreadable,
    )
}

fn sample_batch_report() -> BatchReport {
    BatchReport {
        outcomes: vec![sample_outcome()],
        cancelled: false,
    }
}

fn sample_preset() -> Preset {
    crate::presets::all_presets()
        .first()
        .copied()
        .expect("the preset catalogue is not empty")
}

fn sample_field_layout() -> crate::ffi_abi::FieldLayout {
    crate::ffi_abi::px_buffer_layout()
        .fields
        .first()
        .copied()
        .expect("PxBuffer has fields")
}

fn sample_crop_spec() -> CropSpec {
    CropSpec {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    }
}

fn sample_categories() -> Vec<crate::presets::Category> {
    use crate::presets::Category::*;
    vec![Social, Web, Print, Device, Email, Developer]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The check is worthless if it reports nothing, and a `walk` that silently
    /// stopped recursing would report a plausible-looking subset. Asserted on
    /// shape, not on a count, because the count is supposed to change.
    #[test]
    fn the_contract_is_not_empty_and_covers_every_surface() {
        let lines = contract();
        for owner in [
            "ValidateReport",
            "ExifInfo",
            "ExifEntry",
            "ProcessResponse",
            "ProcessRequest",
            "BatchRequest",
            "BatchFile",
            "Outcome",
            "BatchReport",
            "Capabilities",
            "Preset",
            "Pipeline",
            "ColourProfile",
            "ColourOutcome",
            "ColourOptions",
            "AnimationOutcome",
            "StructLayout",
        ] {
            assert!(
                lines.iter().any(|l| l.starts_with(&format!("{owner}."))),
                "no fields recorded for {owner}"
            );
        }
        assert!(lines.len() > 150, "only {} lines", lines.len());
    }

    /// Nested paths are the part a hand-written list gets wrong, and the part
    /// that makes the check worth having: `ValidateReport.colour` alone would
    /// tell the Dart side to add a field and still leave all five of the
    /// profile's own fields missing.
    #[test]
    fn nested_objects_are_recorded_by_path() {
        let lines = contract();
        for field in [
            "icc_present",
            "source",
            "declared",
            "description",
            "icc_bytes",
        ] {
            assert!(
                lines.contains(&format!("ValidateReport.colour.{field}")),
                "ValidateReport.colour.{field} not recorded"
            );
        }
        assert!(lines.contains(&"ValidateReport.colour.icc_present".to_string()));
        assert!(lines.contains(&"ExifInfo.colour.icc_present".to_string()));
    }

    /// An array of objects has one set of keys, and the caller reaches them
    /// through the list rather than through an index.
    #[test]
    fn a_list_of_objects_contributes_its_element_fields() {
        let lines = contract();
        assert!(lines.contains(&"ExifInfo.entries.tag".to_string()));
        assert!(lines.contains(&"ExifInfo.entries.value".to_string()));
    }

    /// The variants of an internally tagged enum differ in their fields, so one
    /// sample would have missed four of them. Each is asserted by name because
    /// the whole point is that a caller has to reach all of them.
    #[test]
    fn every_skip_reason_variant_contributes_its_own_fields() {
        let lines = contract();
        assert!(lines.contains(&"SkipReason.Duplicate.of".to_string()));
        assert!(lines.contains(&"SkipReason.TooLarge.limit".to_string()));
        assert!(lines.contains(&"SkipReason.TooLarge.actual".to_string()));
        assert!(lines.contains(&"SkipReason.TooLarge.unit".to_string()));
        assert!(lines.contains(&"SkipReason.WouldUpscale.requested".to_string()));
        assert!(lines.contains(&"SkipReason.WouldUpscale.actual".to_string()));
        assert!(lines.contains(&"SkipReason.UnsupportedFormat.format".to_string()));
    }

    /// Wire names come from `rename_all`, so `DisplayP3` is `display_p3`. If
    /// this ever reports the Rust spelling, the check is comparing the wrong
    /// strings and would pass on a contract the app cannot speak.
    #[test]
    fn variant_names_are_the_wire_names_not_the_rust_names() {
        let lines = contract();
        assert!(lines.contains(&"ColourSpace::display_p3".to_string()));
        assert!(!lines.contains(&"ColourSpace::DisplayP3".to_string()));
        assert!(lines.contains(&"ChromaSubsampling::luma444".to_string()));
        assert!(lines.contains(&"OutputFormat::heic".to_string()));
        assert!(lines.contains(&"OutputFormat::heif".to_string()));
        assert!(lines.contains(&"AnimationPolicy::first_frame".to_string()));
        assert!(lines.contains(&"AnimationAction::preserved".to_string()));
        assert!(lines.contains(&"SizeUnit::pixels".to_string()));
        assert!(lines.contains(&"SkipReason::would_upscale".to_string()));
        assert!(lines.contains(&"ResampleFilter::catmull_rom".to_string()));
    }

    /// `#[serde(flatten)]` inlines the pipeline's fields into the batch request,
    /// so `BatchRequest.crop` is a real top-level key and not a nested one. A
    /// generator that did not flatten would report `BatchRequest.pipeline.crop`,
    /// which is a key that does not exist — a check that fails on the correct
    /// code.
    #[test]
    fn a_flattened_field_reports_as_a_top_level_key() {
        let lines = contract();
        assert!(lines.contains(&"BatchRequest.crop".to_string()));
        assert!(!lines.contains(&"BatchRequest.pipeline".to_string()));
    }

    /// The whole output is sorted, because the committed file is diffed and an
    /// unsorted list makes every added field look like a whole-file change.
    #[test]
    fn the_contract_is_sorted_and_deduplicated() {
        let lines = contract();
        let mut sorted = lines.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(lines, sorted);
    }
}
