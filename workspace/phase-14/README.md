# phase-14, app half: skip reasons, deduplication policy, folder counts

`Outcome` gained `skipped`, a batch request gained `policy`, and this patch adds
both to the Dart side: `SkipReason` as a sealed class with one type per answer,
`BatchPolicy` for the three independent switches, and `skipped` / `duplicates`
counters on `BatchReport` so the UI can say "370 of 400" and why.

Apply it to `authorss81/shrinkray` and commit there; this pipeline cannot push to
that repository (403, the same limit as phase-05 and phase-13).

```bash
cd app
git apply ../workspace/phase-14/shrinkray-phase-14.patch
dart format lib test && flutter analyze
bash native/build-engine.sh          # or the engine must be on the loader path
flutter test
git commit -m "phase-14: skip reasons and a batch policy" && git push
```

**This patch does not stack with phase-06's.** Both read a format name out of a
different part of the contract — phase-06's adds `heic`/`heif` to
`OutputFormat`, this one reads whatever format a skip reason names — so their
hunks are in different places. phase-06's also touches the `Capabilities`
constructor, which phase-07's patch touches; of the three, only this one and
phase-06's avoid that constructor.

**One deliberate tolerance, and why.** `SkipReasonUnsupportedFormat` carries the
format twice: as the name the engine sent, and as the `OutputFormat` value if
this app knows it. `OutputFormat.fromJson` throws on a name the enum lacks, which
is right for a request and wrong for a response — an engine that starts
reporting HEIC skips against an app that has not been taught HEIC would crash
every report in the folder rather than render one line of it. The unknown-format
arm returns `null` for `format` and keeps the name for the message. The same
reasoning gives the unknown-`kind` arm a value rather than an exception.

**One thing this patch cannot test yet.** `models_test.dart` exercises
`unsupported_format` with `avif` rather than `heic`, because this pinned
submodule commit's `OutputFormat` enum has no HEIC until phase-06's patch lands.
Naming a format the enum cannot represent would make the test crash for a reason
that has nothing to do with the skip reason it is testing.

Verified on the pinned submodule commit: `dart format` clean, `flutter analyze`
zero issues, `flutter test` **19 passed, none skipped** against the debug engine
built from this repository's `core/`.