# Animated GIF: what this engine does, and why

## The policy

**Keep every frame when the output format can hold them. Refuse the export when
it cannot. Never drop frames without saying so in a field the caller can read.**

Written out as a table, because the interesting part is which cells say what:

| Input | Output format | `AnimationPolicy::Keep` (default) | `AnimationPolicy::FirstFrame` |
| --- | --- | --- | --- |
| Still | anything | written as a still | written as a still |
| Animated | GIF | every frame, with its delay | the first frame, `dropped()` reported |
| Animated | anything else | **nothing written**, refused in a sentence | the first frame, `dropped()` reported |

There is no fourth row, and there is no default that flattens.

It is implemented in `core/src/animation.rs`, and the three types that make it
visible are `AnimationPolicy` (the request), `AnimationAction` (the word for what
happened) and `AnimationOutcome` (the counts, the word and the policy). The
outcome rides on `worker::Outcome` and in the JSON `px_batch` and `px_process`
return, so a caller finds out what happened to the frames without decoding the
output and without parsing the error string.

## Why this one

The phase prompt offered three options. Here is why each was kept or dropped,
with the part of the argument that is not a preference.

### Rejected: flatten, loudly

**This is the behaviour the engine had.** An animated GIF was decoded to its
first frame by `decode_bounded` — `image`'s `ImageReader::decode` on a
multi-frame container hands back the first one and says nothing — and the
`has_animated` flag that `validate_bytes` had been computing sat on the report
unread. Every exported animation was a still JPEG with no warning anywhere.

Adding a warning fixes the silence and none of the problem, for three reasons.

**A warning in a list is not a warning.** The batch report carries two hundred
outcomes and one line each. A toast that says "2 files were animations and were
flattened" is correct, arrives once, and is gone before the user has finished
scrolling. Hard rule 9 asks for failures to be *explained to the user*; a fact
buried in a summary line of a report nobody reads is explained to the log.

**The user cannot act on it afterwards.** The failure mode is not "the app
crashed". It is "I shared a meme and it came out as one frame and my friend
laughed at the wrong thing". Once the file is written, the only remedy is to
find the original again, and the app has thrown away the knowledge of which
files were affected unless the user read the report.

**The count is only half the information.** "This had 240 frames" tells a user
what they lost. "This had 2 frames" does not tell them whether that was an
animation or a JPEG with a funny extension, which is why the outcome carries
`frames_in` as well as an action word.

So flattening is not wrong in itself — a still of the first frame is often
exactly what someone wants from a meme — but it cannot be the *default*. It is
therefore the second policy here, behind an explicit flag, and it reports what
it cost:

```
This file was an animation of 240 frames. The exported image is the first
frame; the other 239 were not written. Choose GIF output to keep every frame.
```

### Rejected: refuse, for everything

Hard refusal is the answer that cannot be wrong about frames, and for a
*lossless* tool like this one it is the boring correct one. It is not the answer
for a *resizer*.

An animated GIF exported as a GIF is not a request to lose anything. The frames
all fit the format; every knob the UI offers applies to them; refusing it would
mean the app cannot resize an animation at all, and the user with 40 frames of
their own animation and no second app goes and uses a second app.

So the refusal is narrowed to the one cell where it is the only honest answer:
**an animation into a format that holds one picture.** JPEG, PNG, WebP, AVIF,
TIFF, BMP and ICO each hold exactly one frame, and no request that arrives here
can make them hold two. Either the frames are dropped or the export is refused,
and refusing costs the user one file that no tool could have produced.

### Chosen: preserve where possible, refuse where not

The middle column of the table, and the reason `AnimationPolicy::Keep` is the
default rather than `FirstFrame`: an animation the format *can* hold goes through
the pipeline frame by frame, and one it cannot is refused rather than quietly
reduced.

This is the option the prompt called the expensive one, and it is worth being
straight about why it was affordable here rather than assuming it was.

**`image`'s GIF decoder already composites.** This is the fact the whole design
rests on, and it was checked in the dependency's source rather than assumed.
`GifFrameIterator::next` (`image-0.25.10/src/codecs/gif.rs`) blends each frame
against a `non_disposed_frame` canvas *honouring the disposal method* —
`Keep`/`Any` keep the pixels, `Background` clears them, `Previous` leaves the
canvas alone — and respects the transparent index by substituting the previous
canvas wherever the incoming alpha is zero. It returns `animation::Frame` with
the **full canvas** at `(0, 0)` and the frame's delay attached.

That matters because disposal is where every naive GIF implementation breaks,
and it is where this one deliberately does not try. The input side of the problem
is the decoder's job and is already correct; doing it a second time is how a
resizer ends up with doubled, half-erased or background-coloured frames. The
output side needs no disposal method at all, because **every frame written is a
complete canvas**: there is nothing for a disposal method to be right about.
`image`'s encoder writes `Background` for every frame regardless, and for a
full-canvas frame that is correct rather than lossy — a transparent pixel in a
composited frame means nothing was ever painted there, so clearing to background
reproduces exactly what the source composited to.

So "preserve" here is genuinely just "run the pipeline once per frame", which is
what `animation::preserve` does.

## How the frames go through the pipeline

```
container walk  →  validate_bytes  →  animation::decide
                                        │
                    ┌───────────────────┼────────────────────┐
                    │                   │                    │
                 Still             Preserved            Flattened / Refused
                    │                   │                    │
                    │            decode frame 1             │
                    │            colour::apply                │
                    │            pipeline.apply              │
                    │            decode frame 2 …             │
                    │            format::encode_frames        │
                    │                   │                    │
                    └──────────► the ordinary single-image path ◄─┘
```

Each frame goes through `colour::apply` then `Pipeline::apply`, which is
crop → orient → resize and **one resampling pass per frame**. That is the
reading of hard rule 5 this phase takes: the rule bans a second pass over the
*same* pixels, and a frame is not a pass over the frame before it.
`resize::resample` therefore still has exactly one call site.

Frame delays are carried, not recomputed: `image::Frame::delay` is read off the
decoded frame and handed to `Frame::from_parts`. It is a millisecond `Ratio`, and
`image`'s encoder divides by ten again on the way out, so a round trip is exact
to the GIF's own resolution of a hundredth of a second. Nothing in the pipeline
touches it, and `an_animation_exported_as_gif_keeps_every_frame_and_its_delay`
asserts the delays coming out match the ones going in.

### Why the whole animation is bounded at once

`Limits::check_animation` applies `max_pixels` to `width × height × frames`,
not per frame. Every resized frame is held in memory while the encoder writes
them, so a 200-frame export costs 200 times what one frame costs — and a per-frame
`check_header` passes every one of them. On `Limits::mobile()` that is 40 MP in
total, so a 40 MP animation is accepted and a 40 MP × 5-frame one is not.

The check runs *before* the first frame is decoded, from the output dimensions
`Pipeline::output_dimensions` predicts and the frame count the container walk
found, so a refusal costs the header walk and nothing else.

## What is refused, and what it says

| Situation | What happens | Why |
| --- | --- | --- |
| Animation into JPEG/PNG/WebP/AVIF/TIFF/BMP/ICO | `Err(Error::AnimationRefused)`, nothing written | the format holds one picture |
| Animation with `frames_truncated` | `Err`, nothing written | the frame count is a lower bound, so "every frame in, every frame out" cannot be checked |
| A frame that fails to decode | `Err`, nothing written | a 99-frame export of a 100-frame GIF reported as a success is the same claim one level up |
| Container claims *n*, decoder produces *m < n* | `Err`, nothing written | same |
| More than the profile's pixel budget in total | `Err(PixelBudgetExceeded)` | hard rule 4 |
| A caller that skipped `decide` and handed `preserve` a still format | `Err(UnsupportedFormat)` | the format layer refuses on its own too |

The refusal sentence names the count, the fact and the alternative, because
those are the three things someone who has just been told "no" needs:

```
This file is an animation of 240 frames, and the format you chose holds one
picture, so nothing was written rather than quietly dropping the rest. Choose
GIF output to keep every frame, or ask for the first frame only if that is what
you want.
```

## Honest answer to the prompt's judgement question

*Is the chosen policy the one a user would thank us for, or the one that is
easiest to implement?*

**Preserving is what a user would thank us for, and it is the option that would
have been most expensive.** The two happen to coincide here only because
`image`'s decoder composes for us. Had it not, the right answer would still be
refuse-over-flatten, and this phase would have shipped the refusal with a named
follow-up rather than a composition engine written badly in a hurry.

Where the policy is *not* what a user would ask for, it says so:

- **A damaged GIF is refused rather than partly exported.** A GIF missing its
  trailer — every frame readable, one byte short — is refused for the animation
  path, because there is no way to check that the frames we read are all of them.
  A user whose viewer opens such a file will find this stricter than they expect.
  `AnimationPolicy::FirstFrame` is the way out.
- **Nothing is written for the refused file**, so a batch of 200 with three
  animations exported into a still format gives 197 files. The refusals are in
  `Outcome.animation` with `action: "refused"` and the frame count, not only in
  the error string, so a UI can list them.
- **Frames are not preserved into anything but GIF.** WebP and AVIF can both
  carry animation and neither is written animated in this build; see
  "Not done" below.

## Not done, and named rather than glossed

- **Animated WebP and APNG.** `count_frames` was a GIF-only walk and still is,
  so an animated WebP or APNG is reported as a single-frame still. That is the
  same bug this phase just fixed, for a format with a multi-frame decoder
  sitting right there in the same crate. It is named rather than hidden because
  animated WebP is *rarer* than animated GIF but not rare, and the fix is the
  same walk over a different container plus `image::codecs::webp::WebPDecoder`
  and `png::Decoder`'s frame iterator. The place to put it is
  `validate::scan_frames`, which is currently a GIF-only function whose name
  says as much.
- **The streaming path does not handle animations.** `streamed_resize` returns
  `None` for an animated input because `stream.rs` has no frame concept, so a
  preserved animation takes the in-memory path. Correct, and it costs the memory
  the `streaming` feature exists to save — for a GIF, which is bounded by
  `check_animation` anyway.
- **`quality_used` is 0 for a preserved animation**, which is the honest report:
  GIF is palette-quantised and `OutputFormat::is_lossless(Gif)` is already
  `true`. A byte ceiling on a GIF is refused by `Settings::validate` before this
  point, so the target search is unreachable on this path.
- **Metadata.** A GIF has no EXIF, so `exif::strip` has nothing to do here and
  the frames are built from decoded samples regardless — which is the same
  property the rule is after. ICC embedding is refused for GIF by
  `colour::apply`, because `OutputFormat::supports_icc(Gif)` is `false`.
- **A frame count ceiling.** Time on the animation path is bounded by the input
  byte limit (each frame costs at least a dozen bytes of container) and memory by
  `check_animation`, so no new `Limits` field was added. A 40 000-frame GIF of
  4×4 pictures is inside both and will take a while; it is not a bomb.