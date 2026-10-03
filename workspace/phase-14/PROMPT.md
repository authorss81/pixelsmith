# phase-14 — Content-hash deduplication and a folder pipeline

**Track:** A — Engine & Security
**Depends on:** phase-13
**Timeout:** 75 minutes

## Objective

Make folder processing bearable. Someone who picks a 400-image folder where 30 are the same photo should get 370 results, not 400, and should be told which 30 were skipped and why.

## Read first

- `AGENTS.md` — hard rule 7. Filenames are attacker-controlled; hashes are not.
- `core/src/worker.rs` — `process_batch`, `Outcome`, and where a skipped file would be reported.

## Scope

### Do

- Add content hashing. Use BLAKE3 rather than SHA-256: it is faster, which matters for 400 files, and there is no compatibility requirement here.
- Hash the **decoded pixel data plus the pipeline parameters**, not the file bytes. Two different JPEGs of the same photo must collide, because they are the same image to a user. A test must prove this: encode the same image at two qualities and assert one hash.
- Add a `skipped` variant to `Outcome` with a reason enum: `duplicate`, `unreadable`, `too_large`, `unsupported_format`, `would_upscale`. Every skip must have a reason. A silent skip is a bug.
- Add folder traversal: given a list of paths, enumerate files, filter by extension *and* by magic bytes, and produce a `FolderPlan` the UI can show before processing. The plan is the preview; processing should never be the first time a user learns what is in the folder.
- Parallelism must be bounded. Uncapped parallelism on a 16-core phone decoding 400 images will thrash and may hit the phase-11 memory ceiling several times over. Size the pool to available memory as well as core count, and comment on why.
- Add tests: two byte-different encodings of one image deduplicate; the same image with two different pipelines does not; every skip reason is reachable by a test; a folder of 200 files with 30 duplicates reports exactly 370 processed and 30 skipped.

### Do not

- Do not deduplicate on filename. Two files called `IMG_0001.jpg` are usually different images.
- Do not skip a file without a recorded reason.
- Do not read an entire large file into memory to hash it. Stream it.

## Acceptance criteria

### Machine-checkable

- A test asserts two different-quality encodings of one image produce one output.
- A test asserts the same image under two pipelines produces two outputs.
- Every `SkipReason` variant is constructed by at least one test.
- A 200-file folder test asserts exactly 370 processed and 30 skipped, with reasons.

### Needs a human judgement

- Would the folder plan give a user enough confidence to press the button without opening the folder first?
- Is a "would_upscale" skip the right call, or should that be a warning rather than a skip? Argue it in `docs/ARCHITECTURE.md`.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.