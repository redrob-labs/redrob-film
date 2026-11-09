# Motion guide for Redrob Film

Read this before building titles, promos or any motion graphics. `scene_build` applies these
rules by default; the notes say how to do the same by hand with `effects.run`.

## Rules

1. **No linear motion.** Every move eases: a strong ease-out on arrival (out influence 80-90 % on
   the first key, in influence ~90 % on the last). By hand: `prop.addKey` twice, then
   `keys.setEase` on each key.
2. **Entrances move two or three properties together** (opacity with position and/or scale). An
   opacity fade on its own looks unfinished.
3. **Stagger.** Elements arrive one after another, 0.1-0.2 s apart (3-6 frames at 30 fps). Nothing
   enters at the same instant as anything else.
4. **Exits are shorter than entrances** (about 0.3 s against 0.5-0.6 s) and ease in.
5. **Hold.** Fast move, then stillness, then the next move. Constant motion reads cheap; contrast
   reads expensive. Leave at least a second of hold on every title.
6. **Layer stack**, bottom to top: background (footage or a gradient, never flat black) → a dark
   wash for contrast → graphics and type → a grade layer (vignette, a little grain).
7. **Footage drifts.** A slow push-in of 5-8 % over the shot (Ken Burns), eased.
8. **One hero.** One hero colour per frame and one glowing element at most. Everything else is
   neutral.
9. **Type.** Hero text bold, 120-170 px on a 1080-wide frame, tight tracking. Body text regular,
   50-60 px, dimmer. Labels small, all caps, wide tracking, in the hero colour.
10. **Vertical safe zone.** On 9:16 keep text between 14 % and 65 % of the height: platform UI
    covers the top 14 % and the bottom third (captions belong above the bottom 35 % too).
11. **Something moves in the first half second.** The hook comes first; a new visual element at
    least every 3 seconds.
12. **Look before you report.** `render_contact_sheet` over the source before cutting, and over the
    result after. Fix what you see (text touching edges, black bars, an element visible before its
    entrance) and look again. Never describe a frame you have not seen.

13. **Transitions: mostly hard cuts.** Trailers and good shorts cut; the cut is the rhythm.
    Spins, pops, flips, cubes and page turns read as a template. Use at most two or three
    transitions in 30 s, each with a reason: `light_leak` or `flash` (6-10 frames) on a hit,
    `film_dissolve` or `blur_dissolve` (12-18 frames) for a change of place or time,
    `dip_to_black` (16-24 frames) before the end card. Never the same one twice in a row.
14. **Endings breathe.** Hold the end card at least 2 s after it lands, then fade to black over
    1-1.5 s (`scene_build` `fadeOut`), and fade the music out over the last 2 s. Never end on a
    cut to black or with the music still at full level.
15. **Sound is half of the feel.** A sound on every major hit, nowhere else: a `reverse` swell and a
    `hit` on the hero title, a `riser` into the payoff with a `boom` on it, a `shimmer` on the end
    card. Accents land on the frame (`sfx_add` places them so). Keep the music bed 3-6 dB down
    under the effects, and do not put a whoosh on every cut.

## 30-second short, a structure that works

| Time | Part | What happens |
|---|---|---|
| 0-1.5 s | Hook | the boldest image and one line; motion inside the first 15 frames |
| 1.5-3 s | Context | one line, one image |
| 3-22 s | Body | 3-4 beats: hit, hold, build |
| 22-27 s | Payoff | the biggest move of the video |
| 27-30 s | Call to action | one action, calm, the hero colour on it |

## Tools

- `scene_build` builds a whole composition from a description (see its schema): background or
  the selected footage, roles (`hero`, `title`, `body`, `label`, `cta`), entrances (`rise`,
  `drop`, `slide`, `pop`, `fade`, `grow`, or a text preset: `trackingIn`, `fadeUpCharacters`,
  `blurIn`, `typewriter`, `bounceInWords`, `scramble`, `dropInLines`), exits, the look
  (`darken`, `vignette`, `grain`, `kenBurns`), `fadeOut` and `place` to put it on the
  timeline. It returns `cues`: the entrance times with a suggested sound for each.
- `sfx_add` places synthesised sound effects (whoosh, hit, riser, shimmer, click, boom,
  reverse) with their accents on the given times; pass it the `cues` of `scene_build`.
- Music: `clip.audioGain` {mode: "set", db: -4} for the bed; for the fade-out,
  `effects.toggleAnimation` then `effects.setParam` on the clip's `volume` / `level` at two times.
- `clip.fillFrame` fills a vertical frame from horizontal footage and crops baked-in letterbox.
- Edit transitions with motion (`sequence.applyVideoTransition`): `pull_motion`,
  `travel_motion`, `spin_motion`, `pop_motion`, `blur_dissolve`, `additive_dissolve`.
- Export short-form vertical video with the preset `Social Vertical 1080×1920`.

Sources these rules draw on: Remotion's motion-graphics agent skill
(github.com/haidrrrry/claude-remotion-skill), open-fusion-mcp's scene builder and render-and-look
loop (github.com/legionsound/open-fusion-mcp), and Meta's Reels safe-zone guidance.
