//! The C ABI, described as data.
//!
//! [`crate::ffi`] holds the implementations; this module holds their *shape*.
//! The two were previously kept in sync by a human reading both files and
//! hoping, which is not a mechanism: a `px_*` function can gain a parameter, and
//! the Dart declaration keeps the old arity, and nothing anywhere notices. The
//! result is not a compile error on either side — it is a call with the wrong
//! number of arguments across the FFI boundary.
//!
//! So each entry point is declared exactly once, here, in the macro call at the
//! bottom of this file. That one declaration does two jobs:
//!
//! 1. It becomes a `const _: unsafe extern "C" fn(..) -> ..` assignment, which
//!    only compiles if the listed types really are that function's signature.
//!    Adding a parameter to `px_inspect` and forgetting this file is a build
//!    failure, not a runtime surprise.
//! 2. It is rendered into the Dart declarations `bindings.dart` must contain,
//!    and [`ENTRY_POINTS`] is what the drift test compares against that file.
//!
//! Nothing here is a second implementation of the boundary. It is the boundary's
//! type signature, written once.

use crate::ffi::{PxBuffer, PxHandle};
use std::ffi::c_char;
use std::mem::{align_of, offset_of, size_of};

/// One `px_*` entry point, as the Dart side has to declare it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryPoint {
    /// The exported symbol name, spelled the way a C header would.
    pub symbol: &'static str,
    /// Dart *native* parameter types, in declaration order.
    ///
    /// These are the types that must appear inside the `Function(...)` of the
    /// native half of `lookupFunction`, not the Dart-side ones: a mismatch there
    /// is what the VM checks at call time.
    pub params: &'static [&'static str],
    /// The Dart native return type.
    pub result: &'static str,
    /// Dart-side parameter types, for the second half of `lookupFunction`.
    pub dart_params: &'static [&'static str],
    /// The Dart-side return type.
    pub dart_result: &'static str,
}

impl EntryPoint {
    /// Number of arguments the caller must pass.
    pub fn arity(&self) -> usize {
        self.params.len()
    }

    /// True when the caller receives a [`PxBuffer`] and therefore owns memory
    /// that has to be handed back with `px_buffer_free`.
    pub fn returns_owned_buffer(&self) -> bool {
        self.result == <PxBuffer as ToDart>::NATIVE
    }

    /// `px_inspect(ffi.Pointer<ffi.Uint8>, ffi.Size, ffi.Bool) -> PxBuffer`
    ///
    /// One line per entry point, for reading and for diffing.
    pub fn signature_line(&self) -> String {
        format!(
            "{}({}) -> {}",
            self.symbol,
            self.params.join(", "),
            self.result
        )
    }
}

/// A Rust type that crosses the boundary, and the two Dart spellings it needs.
///
/// `NATIVE` is what appears in the native half of `lookupFunction` and is what
/// the VM checks against the real machine code. `DART` is what Dart itself
/// accepts. They differ for pointers — `ffi.Size` is `int` in Dart — and
/// agreeing on neither is how a hand-written binding rots.
pub trait ToDart {
    /// The `dart:ffi` native type, e.g. `ffi.Pointer<ffi.Uint8>`.
    const NATIVE: &'static str;
    /// The Dart type an argument of this kind is passed as.
    const DART: &'static str;
}

impl ToDart for u8 {
    const NATIVE: &'static str = "ffi.Uint8";
    const DART: &'static str = "int";
}

impl ToDart for u32 {
    const NATIVE: &'static str = "ffi.Uint32";
    const DART: &'static str = "int";
}

impl ToDart for usize {
    const NATIVE: &'static str = "ffi.Size";
    const DART: &'static str = "int";
}

impl ToDart for bool {
    const NATIVE: &'static str = "ffi.Bool";
    const DART: &'static str = "bool";
}

/// Opaque, never dereferenced by Dart — see the handle rules in [`crate::ffi`].
impl ToDart for PxHandle {
    const NATIVE: &'static str = "ffi.Uint64";
    const DART: &'static str = "int";
}

impl ToDart for PxBuffer {
    const NATIVE: &'static str = "PxBuffer";
    const DART: &'static str = "PxBuffer";
}

impl ToDart for *const u8 {
    const NATIVE: &'static str = "ffi.Pointer<ffi.Uint8>";
    const DART: &'static str = "ffi.Pointer<ffi.Uint8>";
}

impl ToDart for *mut u8 {
    const NATIVE: &'static str = "ffi.Pointer<ffi.Uint8>";
    const DART: &'static str = "ffi.Pointer<ffi.Uint8>";
}

impl ToDart for *mut c_char {
    const NATIVE: &'static str = "ffi.Pointer<ffi.Char>";
    const DART: &'static str = "ffi.Pointer<ffi.Char>";
}

impl ToDart for () {
    const NATIVE: &'static str = "ffi.Void";
    const DART: &'static str = "void";
}

/// Where one field sits inside a `#[repr(C)]` struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct FieldLayout {
    pub name: &'static str,
    pub offset: usize,
    pub size: usize,
}

/// The size, alignment and field offsets of a struct Dart reads by offset.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StructLayout {
    pub name: &'static str,
    pub size: usize,
    pub align: usize,
    pub fields: Vec<FieldLayout>,
}

/// The layout of [`PxBuffer`], computed rather than remembered.
///
/// Dart indexes this struct by byte offset, so a wrong offset reads another
/// field's bytes as a pointer. Publishing the numbers lets `px_abi_layout` answer
/// "is your declaration right?" at runtime, instead of the Dart test asserting a
/// number somebody typed into a comment months earlier.
pub fn px_buffer_layout() -> StructLayout {
    StructLayout {
        name: "PxBuffer",
        size: size_of::<PxBuffer>(),
        align: align_of::<PxBuffer>(),
        fields: vec![
            FieldLayout {
                name: "status",
                offset: offset_of!(PxBuffer, status),
                size: size_of::<u32>(),
            },
            FieldLayout {
                name: "data",
                offset: offset_of!(PxBuffer, data),
                size: size_of::<*mut u8>(),
            },
            FieldLayout {
                name: "len",
                offset: offset_of!(PxBuffer, len),
                size: size_of::<usize>(),
            },
            FieldLayout {
                name: "error",
                offset: offset_of!(PxBuffer, error),
                size: size_of::<*mut c_char>(),
            },
        ],
    }
}

/// Declare every entry point once.
///
/// Each row expands to a compile-time check plus a table row, so the table cannot
/// describe a function that does not exist or has a different signature. Keep
/// the order the same as `ffi.rs`; it is the order the documentation lists them
/// in, and diffing two ordered lists is the whole point.
macro_rules! entry_points {
    ($( $name:ident( $( $arg:ty ),* ) -> $ret:ty ; )*) => {
        $(
            #[doc = concat!("Proves the table row below really is `", stringify!($name), "`.")]
            const _: unsafe extern "C" fn( $( $arg ),* ) -> $ret = crate::ffi::$name;
        )*

        /// Every `px_*` entry point, in `ffi.rs` declaration order.
        pub const ENTRY_POINTS: &[EntryPoint] = &[
            $(
                EntryPoint {
                    symbol: stringify!($name),
                    params: &[ $( <$arg as ToDart>::NATIVE ),* ],
                    result: <$ret as ToDart>::NATIVE,
                    dart_params: &[ $( <$arg as ToDart>::DART ),* ],
                    dart_result: <$ret as ToDart>::DART,
                },
            )*
        ];
    };
}

entry_points! {
    // Lifecycle.
    px_version() -> PxBuffer;
    px_buffer_free(PxBuffer) -> ();
    px_string_free(*mut c_char) -> ();

    // Inspection.
    px_inspect(*const u8, usize, bool) -> PxBuffer;
    px_exif(*const u8, usize) -> PxBuffer;
    px_presets() -> PxBuffer;

    // Processing.
    px_process(*const u8, usize) -> PxBuffer;

    // Cancellation. No buffers, so nothing to free.
    px_cancel_new() -> PxHandle;
    px_cancel_trigger(PxHandle) -> bool;
    px_cancel_free(PxHandle) -> bool;

    px_batch(*const u8, usize) -> PxBuffer;
    px_zip(*const u8, usize) -> PxBuffer;

    // Deliberate failures, for the host's own tests.
    px_selftest_error() -> PxBuffer;
    px_selftest_panic() -> PxBuffer;

    // The layout of `PxBuffer`, so the host can check its struct declaration.
    px_abi_layout() -> PxBuffer;
}

/// The Dart declarations `bindings.dart` has to contain.
///
/// This is a reference, not a drop-in replacement for the file: it is what the
/// drift test compares against, and what a human pastes in when they regenerate
/// the bindings. `cargo run --bin px-abi-dump` prints it.
pub fn dart_binding_declarations() -> String {
    let mut out = String::new();
    for entry in ENTRY_POINTS {
        out.push_str(&format!(
            "  late final {field} = _lib.lookupFunction<\n    {native} Function({native_params}),\n    {dart} Function({dart_params})\n  >('{symbol}');\n",
            field = dart_field_name(entry.symbol),
            native = entry.result,
            native_params = entry.params.join(", "),
            dart = entry.dart_result,
            dart_params = entry.dart_params.join(", "),
            symbol = entry.symbol,
        ));
    }
    out
}

/// `px_buffer_free` becomes `_pxBufferFree`: the private field name the
/// existing `bindings.dart` uses for each lookup.
fn dart_field_name(symbol: &str) -> String {
    let mut name = String::from("_");
    let mut capitalise = false;
    for c in symbol.chars() {
        if c == '_' {
            capitalise = true;
            continue;
        }
        if capitalise {
            name.extend(c.to_uppercase());
            capitalise = false;
        } else {
            name.push(c);
        }
    }
    name
}

/// How many arguments the Dart declaration of `symbol` claims, if it has one.
///
/// `bindings.dart` is hand-written and `dart format` wraps it across lines, so
/// this reads the declaration the way the VM does: find the symbol, walk back to
/// its `lookupFunction<`, and count the parameters of the native function type.
/// Anything it cannot parse comes back as `None`, which the caller reports as
/// drift rather than as agreement.
pub fn dart_arity(source: &str, symbol: &str) -> Option<usize> {
    let needle = format!("'{symbol}'");
    let at = source.find(&needle)?;
    let block_start = source[..at].rfind("lookupFunction<")?;
    declared_arity(&source[block_start..at])
}

/// Parameter count of the first `Native Function(..)` type in `block`.
fn declared_arity(block: &str) -> Option<usize> {
    const OPEN: &str = "Function(";
    let start = block.find(OPEN)? + OPEN.len();
    let tail = &block[start..];
    let mut depth = 0usize;
    let mut commas = 0usize;
    let mut non_empty = false;
    for c in tail.chars() {
        match c {
            '(' | '<' | '[' => {
                depth += 1;
                non_empty = true;
            }
            ')' | '>' | ']' => {
                if depth == 0 {
                    return Some(usize::from(non_empty) + commas);
                }
                depth -= 1;
            }
            ',' if depth == 0 => commas += 1,
            c if !c.is_whitespace() => non_empty = true,
            _ => {}
        }
    }
    None
}

/// Every way `source` — a Dart `bindings.dart` — disagrees with the engine ABI.
///
/// One line per problem, in [`ENTRY_POINTS`] order, so the output diffs cleanly
/// against [`ENTRY_POINTS`] when a signature changes. An empty result means the
/// two sides agree.
pub fn dart_drift(source: &str) -> Vec<String> {
    let mut drift = Vec::new();
    for entry in ENTRY_POINTS {
        match dart_arity(source, entry.symbol) {
            None => drift.push(format!("{}: no Dart declaration", entry.symbol)),
            Some(n) if n != entry.arity() => drift.push(format!(
                "{}: Dart declares {n} arguments, the engine takes {}",
                entry.symbol,
                entry.arity()
            )),
            Some(_) => {}
        }
    }
    drift
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path to the Flutter app, which is a submodule of this repository.
    fn bindings_source() -> Option<String> {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
        std::fs::read_to_string(repo.join("app/lib/rust/bindings.dart")).ok()
    }

    /// Symbol names of every `extern "C"` function in `ffi.rs`, in source order.
    ///
    /// Scanned rather than re-declared so that a new entry point cannot be added
    /// without showing up here. `ffi.rs` is the only place a `px_*` symbol is
    /// created, so this cannot miss one that is exported.
    fn exported_symbols() -> Vec<String> {
        const MARKER: &str = "extern \"C\" fn px_";
        let source = include_str!("ffi.rs");
        let lines: Vec<&str> = source.lines().collect();
        let mut found = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("pub ") || !trimmed.contains(MARKER) {
                continue;
            }
            // `#[unsafe(no_mangle)]` and `#[no_mangle]` both appear on their own
            // line just above; a helper function without one must not be counted
            // as part of the ABI.
            let attributed = lines[i.saturating_sub(3)..i]
                .iter()
                .any(|l| l.contains("no_mangle"));
            assert!(
                attributed,
                "{} is exported but has no #[no_mangle]; it is not part of the ABI",
                trimmed
            );
            let rest = &trimmed[trimmed.find(MARKER).expect("marker present") + MARKER.len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            found.push(format!("px_{name}"));
        }
        found
    }

    /// Drift between the engine's ABI and the Dart bindings, known and documented.
    ///
    /// Every entry here is a real defect in `app/`, which is a separate
    /// repository this one cannot push to. They are listed rather than asserted
    /// away so that this test keeps its value: when the app side is fixed the
    /// drift shrinks, this list goes stale, and the test below says so. A test
    /// that skipped the comparison instead would be green forever and would catch
    /// nothing.
    ///
    /// See `workspace/phase-05/FINDINGS.md`.
    const KNOWN_DART_DRIFT: &[&str] = &[
        "px_inspect: Dart declares 2 arguments, the engine takes 3",
        "px_abi_layout: no Dart declaration",
    ];

    #[test]
    fn every_exported_symbol_is_in_the_table() {
        let exported = exported_symbols();
        let tabled: Vec<&str> = ENTRY_POINTS.iter().map(|e| e.symbol).collect();
        assert_eq!(
            exported, tabled,
            "the ABI table and ffi.rs disagree; a px_* function was added or removed in one place only"
        );
    }

    #[test]
    fn the_table_has_no_duplicate_symbols() {
        let mut symbols: Vec<&str> = ENTRY_POINTS.iter().map(|e| e.symbol).collect();
        let before = symbols.len();
        symbols.sort_unstable();
        symbols.dedup();
        assert_eq!(symbols.len(), before, "a px_* symbol is listed twice");
    }

    #[test]
    fn everything_the_engine_owns_can_be_handed_back() {
        // A caller reading only this table has to be able to work out what to
        // free: every function that returns a `PxBuffer` hands over both a data
        // pointer and an error string, so both free functions must exist, and
        // neither may itself return something owned.
        let symbols: Vec<&str> = ENTRY_POINTS.iter().map(|e| e.symbol).collect();
        for free in ["px_buffer_free", "px_string_free"] {
            assert!(
                symbols.contains(&free),
                "{free} is missing, so a caller cannot release what the others hand it"
            );
            let entry = ENTRY_POINTS
                .iter()
                .find(|e| e.symbol == free)
                .expect("listed above");
            assert!(
                !entry.returns_owned_buffer(),
                "{free} returns owned memory, so freeing it would need another free"
            );
        }
        // Everything that owns memory says so through the same result type, so a
        // caller can free without knowing which function it called.
        let owners: Vec<&str> = ENTRY_POINTS
            .iter()
            .filter(|e| e.returns_owned_buffer())
            .map(|e| e.symbol)
            .collect();
        assert!(
            owners.contains(&"px_presets"),
            "nothing returns a buffer, so the ownership rules are untested"
        );
        for symbol in owners {
            assert!(
                !symbol.ends_with("_free"),
                "{symbol} hands back memory and is named like a free function"
            );
        }
    }

    #[test]
    fn px_buffer_layout_is_self_consistent() {
        let layout = px_buffer_layout();
        assert_eq!(layout.name, "PxBuffer");
        assert_eq!(
            layout.fields.len(),
            4,
            "a field was added to PxBuffer without a layout entry"
        );
        for (i, field) in layout.fields.iter().enumerate() {
            if i > 0 {
                let prev = layout.fields[i - 1];
                assert!(
                    field.offset >= prev.offset + prev.size,
                    "{} overlaps {}",
                    field.name,
                    prev.name
                );
            }
        }
        let last = layout.fields.last().expect("at least one field");
        assert_eq!(
            layout.size,
            last.offset + last.size,
            "the last field does not end the struct"
        );
        // `data` is a pointer, so it cannot start inside the u32 that precedes it.
        let status = layout.fields[0];
        let data = layout.fields[1];
        assert_eq!(
            data.offset % std::mem::align_of::<*mut u8>(),
            0,
            "data is not pointer-aligned"
        );
        assert!(
            data.offset >= status.offset + status.size,
            "data starts inside status"
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn px_buffer_is_32_bytes_on_64_bit_targets() {
        // The numbers `bindings.dart` and every Dart-side test are written
        // against. If this changes, the Dart struct is wrong and the host is
        // about to read a length as a pointer.
        let layout = px_buffer_layout();
        assert_eq!(layout.size, 32);
        assert_eq!(layout.align, 8);
        let offsets: Vec<(&str, usize)> =
            layout.fields.iter().map(|f| (f.name, f.offset)).collect();
        assert_eq!(
            offsets,
            vec![("status", 0), ("data", 8), ("len", 16), ("error", 24)]
        );
    }

    #[test]
    fn the_generated_dart_declarations_name_every_symbol() {
        let generated = dart_binding_declarations();
        for entry in ENTRY_POINTS {
            assert!(
                generated.contains(&format!("'{}'", entry.symbol)),
                "{} is missing from the generated bindings",
                entry.symbol
            );
        }
        assert_eq!(
            generated.matches("lookupFunction<").count(),
            ENTRY_POINTS.len(),
            "one lookupFunction per entry point"
        );
    }

    #[test]
    fn the_generated_arities_match_the_table() {
        // Read the generated text back with the same parser used against
        // `bindings.dart`, so the generator is checked by the same rule as the
        // file it is meant to replace. A generator that emits the wrong arity
        // would otherwise be a drift nobody notices, just further from the edge.
        let generated = dart_binding_declarations();
        for entry in ENTRY_POINTS {
            assert_eq!(
                dart_arity(&generated, entry.symbol),
                Some(entry.arity()),
                "{} should take {} arguments",
                entry.symbol,
                entry.arity()
            );
        }
    }

    #[test]
    fn dart_bindings_match_the_engine_abi() {
        let Some(source) = bindings_source() else {
            // The app/ submodule is not checked out. `git submodule update
            // --init --recursive` gives this test something to compare against.
            eprintln!("app/ submodule not present; no Dart bindings to check");
            return;
        };
        let drift = dart_drift(&source);
        assert_eq!(
            drift, KNOWN_DART_DRIFT,
            "the Dart bindings and the engine ABI have drifted. Either fix \
             `app/lib/rust/bindings.dart` (the expected declarations are in \
             `dart_binding_declarations()`) or, if the drift is already known and \
             recorded in workspace/phase-05/FINDINGS.md, update KNOWN_DART_DRIFT."
        );
    }
}
