# Source origins

한국어 요약은 아래에 있습니다.

Redrob Film's code came from two MIT OR Apache-2.0 projects and is no longer synced with either:

| Part | Came from | At |
|---|---|---|
| Edit mode, the app, the CLI (`apps/`, `crates/`) | [FilmCraft](https://github.com/storytold/filmcraft) | tag `v0.6.0`, commit `2ff3f5e266838700c292a35aaf762e3d4dcef980` |
| Effects editor (`editors/effects/`) | [EffectCraft](https://github.com/storytold/effectcraft) | commit `0311bdc34d0dc10c2d18ff7875b4f52d483e5e27` |

From those commits on, the code is developed here; nothing is pulled from or merged with the
original repositories. The exceptions are the ports listed below, taken by hand onto our tree.

## Ports taken later

| Part | Came from | Upstream changes |
|---|---|---|
| After Effects project reader and importer (`editors/effects/crates/aep/`, `file.importAep`, the Effects editor's import prompt `panels/aep_losses.rs`, `editors/effects/docs/aep-format.md`, `editors/effects/docs/aep-import.md`) | [EffectCraft](https://github.com/storytold/effectcraft) `crates/aep/src` at commit `93cce09f3cc248d10360dd8a6dd86d658ce3747e` | #2558 (`bb78d55825adc879ae522c18ac57c4ce5690318d`, the reader and importer), #3134 (`3768ff40a300758e9055ab87bfaaeea6d4bd774e`, effect values stored as default-state records), #2594 (`ce9eaec65015a5508c72af0e2351cd02612c9450`, only its small changes to the shared container and import files; its export module is not ported), #3119 (`ab3674cc2b48d312f55d2966f311b016e56cb5ca`, five more effect mappings), #3136 (`93cce09f3cc248d10360dd8a6dd86d658ce3747e`, the After Effects version in the report) |
| GPU readbacks that copy the mapped rows once and wait only for their own submission (`editors/effects/crates/gpu/src/context.rs` `read_mapped`, `read_texture_with`; `readback.rs` `rows`), and the measured choice of compositor per job, applied to a placed composition (`crates/handoff/src/lib.rs` `CompSource::compositor`, `pick_compositor`) | [EffectCraft](https://github.com/storytold/effectcraft) | #3335 (`4649c27ef97ea649845243500eac1c109c3d4dfc`, its GPU readback part and `pick_renderer`; its Render Queue pipelining, shared caches and the engine and UI changes are not ported) |

Left out of the After Effects port: upstream's `.aep` export (#2594, `crates/aep/src/export`), its test corpus,
fixture generator and `xtask aep-corpus`, the developer examples (`aepdump`, `stubicc`) and the
integration tests that need the corpus. Upstream's document-path policy (`doc_path`) is not in this
tree, so `file.importAep` finds stored footage paths the way `file.importProject` does. Upstream's
crate and docs renamed `EffectCraft` to `Redrob Effects` and `effectcraft-*` to `redrob-effects-*`. Their copyright lines stay unchanged in [`NOTICE`](./NOTICE),
[`LICENSE-MIT`](./LICENSE-MIT), `editors/effects/NOTICE` and `editors/effects/LICENSE-*`, with ours
added beside them. Per-asset attribution stays in [`ATTRIBUTION.md`](./ATTRIBUTION.md) and
`editors/effects/ATTRIBUTION.md`.

## Old names still read

The product's names are Redrob Film's everywhere. A few of the original spellings are stored in
files people already have, or are part of an interface other programs use, so they are kept. Each
place in the code is marked `// legacy:`, and `scripts/check_brand.py` rejects any unmarked one.

- Format ids in saved JSON: `filmcraft.project`, `filmcraft.archive`, `filmcraft.export-presets`,
  `filmcraft.effect-presets`, `filmcraft.graphicsTemplate`, `filmcraft.render-script`,
  `filmcraft.render-log`, `filmcraft-keyboard-shortcuts`; in the Effects editor
  `effectcraft.animationPresets/1`, `effectcraft.textAnimatorPresets/1`, `effectcraftAnimationPreset`,
  `effectcraft-template`, `effectcraft-shortcuts`, `effectcraftGuides`, `effectcraft.panel`.
- The command id `help.filmcraftHelp` (saved keyboard shortcuts name it).
- Interchange: the OTIO metadata namespace `filmcraft`, the FCP 7 XML effect id prefix `filmcraft:`,
  the FCPXML effect uid prefix `filmcraft.`, the AAF tagged values `FilmCraft Frame Size`,
  `FilmCraft Audio Sample Rate`, `FilmCraft Effect`, the OMF properties `FilmCraft:SCLP:ClipName` and
  `FilmCraft:EFFE:EffectID`, OTIO generator kinds `FilmCraftGenerator…`, and the XMP `CreatorTool`
  marker `FilmCraft` that older packets carry.
- Renderer names stored in project settings: `FilmCraft GPU Acceleration (wgpu)`,
  `FilmCraft Software Only (CPU)`; the MXF product UID hash input `FilmCraft MXF writer`.
- Cache keys (changing them only drops caches): `filmcraft-preview-v1`, `effectcraft-render-2`.
- Old file extensions, URI schemes and the XML root, read but never written: `.fcproj`, `.ecproj`,
  `.ecprojx`, `filmcraft://…`, `effectcraft://…`, `<EffectCraftProject>`.
- Environment variables still read as fallbacks: `FILMCRAFT_DATA_DIR`, `FILMCRAFT_CONFIG_DIR`,
  `FILMCRAFT_CONTROL_PORT`, `FILMCRAFT_CPU_COMPOSITE`, `EFFECTCRAFT_CONFIG_DIR`,
  `EFFECTCRAFT_FONT_DIRS`.
- Interfaces: the MCP `_meta` key `effectcraftAutoSave`, and the BridgeTalk app specifier
  `effectcraft` that user scripts address.
- The Effects editor's Linux settings folder `~/.config/effectcraft` (macOS and Windows already use
  `Redrob Film`).

Test fixtures under `crates/format/tests/fixtures/` and `editors/effects/crates/*/tests/fixtures/`
are old files kept as they were written.

## 한국어 요약

레드롭 필름의 코드는 FilmCraft(`v0.6.0`, `2ff3f5e`)와 EffectCraft(`0311bdc`)에서 왔고, 그 뒤로는 이
저장소에서 독자적으로 개발하며 원 저장소와 동기화하지 않습니다. 예외로 After Effects 프로젝트 읽기·가져오기
(`editors/effects/crates/aep/`)는 EffectCraft `93cce09`(#2558, #3134, #3119, #3136)에서 손으로 옮겨 왔고,
`.aep` 내보내기와 테스트 자료는 옮기지 않았습니다. 원 저작권 표기는 `NOTICE`와 `LICENSE-*`에
그대로 두고 우리 줄을 덧붙였습니다. 위 목록의 옛 이름은 사용자가 이미 가진 파일이나 다른 프로그램이 쓰는
인터페이스에 들어 있어 남겨 두었고, 코드에서는 모두 `// legacy:`로 표시했습니다.
