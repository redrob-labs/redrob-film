//! Premiere Pro project (`.prproj`) import. The fixtures are small documents written by hand for
//! these tests (in the shape of the format: a flat pool of `ObjectID` / `ObjectUID` objects joined
//! by `ObjectRef` / `ObjectURef`); no Premiere-authored file is in the repository.
//!
//! `REDROB_FILM_PRPROJ_DIR=<dir>` additionally imports every file in that directory (real
//! projects kept outside the repository) and checks the sequence and clip counts.

use std::collections::HashSet;
use std::io::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

use redrob_film_interchange::{Format, Imported, Report, detect, import, prproj};
use redrob_film_project::{BinEntry, ItemKind, Label, MarkerKind, TransitionAlign};
use redrob_film_time::{FrameRate, TICKS_PER_SECOND, Tick};

const S: i64 = TICKS_PER_SECOND;

/// One sequence "Main" (25 fps, 1920×1080, 48 kHz) with two video clips joined by a cross
/// dissolve, a linked audio clip with clip gain and Volume keyframes, Motion and Opacity, a
/// sequence marker, a caption track, an unknown effect; a "Footage" bin with the media, an
/// offline file and a graphic.
const PROJECT: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<PremiereData Version="3">
	<Project ObjectRef="1"/>
	<Project ObjectID="1" ClassID="test" Version="45">
		<RootProjectItem ObjectURef="root"/>
	</Project>
	<RootProjectItem ObjectUID="root" ClassID="test" Version="1">
		<ProjectItem Version="1"><Name>Root Bin</Name></ProjectItem>
		<ProjectItemContainer Version="1"><Items Version="1">
			<Item Index="1" ObjectURef="pi-main"/>
			<Item Index="0" ObjectURef="bin-footage"/>
			<Item Index="2" ObjectURef="pi-gfx"/>
		</Items></ProjectItemContainer>
	</RootProjectItem>
	<BinProjectItem ObjectUID="bin-footage" ClassID="test" Version="1">
		<ProjectItem Version="1"><Name>Footage</Name></ProjectItem>
		<ProjectItemContainer Version="1"><Items Version="1">
			<Item Index="0" ObjectURef="pi-shot"/>
			<Item Index="1" ObjectURef="pi-off"/>
		</Items></ProjectItemContainer>
	</BinProjectItem>
	<ClipProjectItem ObjectUID="pi-shot" ClassID="test" Version="1">
		<ProjectItem Version="1"><Node Version="1"><Properties Version="1"><Column.PropertyText.Label>BE.Prefs.LabelColors.5</Column.PropertyText.Label></Properties></Node><Name>shot.mov</Name></ProjectItem>
		<MasterClip ObjectURef="mc-shot"/>
	</ClipProjectItem>
	<MasterClip ObjectUID="mc-shot" ClassID="test" Version="10">
		<Clips Version="1"><Clip Index="0" ObjectRef="10"/><Clip Index="1" ObjectRef="11"/></Clips>
		<Name>shot.mov</Name>
	</MasterClip>
	<VideoClip ObjectID="10" ClassID="test" Version="11">
		<Clip Version="18"><MarkerOwner Version="1"><Markers ObjectRef="20"/></MarkerOwner><Source ObjectRef="12"/><InPoint>0</InPoint></Clip>
	</VideoClip>
	<AudioClip ObjectID="11" ClassID="test" Version="8">
		<Clip Version="18"><Source ObjectRef="13"/></Clip>
	</AudioClip>
	<VideoMediaSource ObjectID="12" ClassID="test" Version="2">
		<MediaSource Version="4"><Media ObjectURef="media-shot"/></MediaSource>
		<OriginalDuration>2540160000000</OriginalDuration>
	</VideoMediaSource>
	<AudioMediaSource ObjectID="13" ClassID="test" Version="2">
		<MediaSource Version="4"><Media ObjectURef="media-shot"/></MediaSource>
	</AudioMediaSource>
	<Media ObjectUID="media-shot" ClassID="test" Version="27">
		<VideoStream ObjectRef="14"/>
		<AudioStream ObjectRef="15"/>
		<FilePath>/media/shot.mov</FilePath>
		<Title>shot.mov</Title>
	</Media>
	<VideoStream ObjectID="14" ClassID="test" Version="18">
		<FrameRate>10160640000</FrameRate>
		<FrameRect>0,0,1920,1080</FrameRect>
		<Duration>2540160000000</Duration>
	</VideoStream>
	<AudioStream ObjectID="15" ClassID="test" Version="7">
		<FrameRate>5292000</FrameRate>
		<Duration>2540160000000</Duration>
		<AudioChannelLayout>[{"channellabel":100},{"channellabel":101}]</AudioChannelLayout>
	</AudioStream>
	<Markers ObjectID="20" ClassID="test" Version="3">
		<Markers Version="1"><Marker Version="1" Index="0"><First>m1</First><Second ObjectRef="21"/></Marker></Markers>
	</Markers>
	<Marker ObjectID="21" ClassID="test" Version="1">
		<DVAMarker>{"DVAMarker":{"mName":"take 2","mStartTime":{"ticks":254016000000},"mType":"Comment"}}</DVAMarker>
	</Marker>
	<ClipProjectItem ObjectUID="pi-off" ClassID="test" Version="1">
		<ProjectItem Version="1"><Name>lost.mp4</Name></ProjectItem>
		<MasterClip ObjectURef="mc-off"/>
	</ClipProjectItem>
	<MasterClip ObjectUID="mc-off" ClassID="test" Version="10">
		<Clips Version="1"><Clip Index="0" ObjectRef="30"/></Clips>
	</MasterClip>
	<VideoClip ObjectID="30" ClassID="test" Version="11"><Clip Version="18"><Source ObjectRef="31"/></Clip></VideoClip>
	<VideoMediaSource ObjectID="31" ClassID="test" Version="2"><MediaSource Version="4"><Media ObjectURef="media-off"/></MediaSource></VideoMediaSource>
	<Media ObjectUID="media-off" ClassID="test" Version="27">
		<VideoStream ObjectRef="14"/>
		<FilePath>C:\gone\lost.mp4</FilePath>
		<OfflineReason>1</OfflineReason>
	</Media>
	<ClipProjectItem ObjectUID="pi-gfx" ClassID="test" Version="1">
		<ProjectItem Version="1"><Name>Graphic</Name></ProjectItem>
		<MasterClip ObjectURef="mc-gfx"/>
	</ClipProjectItem>
	<MasterClip ObjectUID="mc-gfx" ClassID="test" Version="10"><Clips Version="1"><Clip Index="0" ObjectRef="70"/></Clips></MasterClip>
	<VideoClip ObjectID="70" ClassID="test" Version="11"><Clip Version="18"><Source ObjectRef="71"/></Clip></VideoClip>
	<VideoMediaSource ObjectID="71" ClassID="test" Version="2"><MediaSource Version="4"><Media ObjectURef="media-gfx"/></MediaSource></VideoMediaSource>
	<Media ObjectUID="media-gfx" ClassID="test" Version="27"><VideoStream ObjectRef="14"/><FilePath>1234567</FilePath><Title>Graphic</Title></Media>
	<ClipProjectItem ObjectUID="pi-main" ClassID="test" Version="1">
		<ProjectItem Version="1"><Name>Main</Name></ProjectItem>
		<MasterClip ObjectURef="mc-main"/>
	</ClipProjectItem>
	<MasterClip ObjectUID="mc-main" ClassID="test" Version="10"><Clips Version="1"><Clip Index="0" ObjectRef="40"/></Clips></MasterClip>
	<VideoClip ObjectID="40" ClassID="test" Version="11"><Clip Version="18"><Source ObjectRef="41"/></Clip></VideoClip>
	<VideoSequenceSource ObjectID="41" ClassID="test" Version="3"><SequenceSource Version="4"><Sequence ObjectURef="seq-main"/></SequenceSource></VideoSequenceSource>
	<Sequence ObjectUID="seq-main" ClassID="test" Version="11">
		<MarkerOwner Version="1"><Markers ObjectRef="50"/></MarkerOwner>
		<PersistentGroupContainer Version="1"><LinkContainer Version="1"><Links Version="1"><Link Index="0" ObjectRef="51"/></Links></LinkContainer></PersistentGroupContainer>
		<TrackGroups Version="1">
			<TrackGroup Version="1" Index="0"><First>video</First><Second ObjectRef="60"/></TrackGroup>
			<TrackGroup Version="1" Index="1"><First>audio</First><Second ObjectRef="61"/></TrackGroup>
			<TrackGroup Version="1" Index="2"><First>data</First><Second ObjectRef="62"/></TrackGroup>
		</TrackGroups>
		<Name>Main</Name>
	</Sequence>
	<Markers ObjectID="50" ClassID="test" Version="3">
		<Markers Version="1"><Marker Version="1" Index="0"><First>m2</First><Second ObjectRef="52"/></Marker></Markers>
	</Markers>
	<Marker ObjectID="52" ClassID="test" Version="1">
		<DVAMarker>{"DVAMarker":{"mName":"Hello","mComment":"note","mStartTime":{"ticks":254016000000},"mDuration":{"ticks":127008000000},"mType":"Chapter"}}</DVAMarker>
	</Marker>
	<Link ObjectID="51" ClassID="test" Version="1">
		<TrackItemGroup Version="1"><TrackItems Version="1"><TrackItem Index="0" ObjectRef="100"/><TrackItem Index="1" ObjectRef="200"/></TrackItems></TrackItemGroup>
	</Link>
	<VideoTrackGroup ObjectID="60" ClassID="test" Version="11">
		<TrackGroup Version="1"><Tracks Version="1"><Track Index="0" ObjectURef="vt1"/></Tracks><FrameRate>10160640000</FrameRate></TrackGroup>
		<FrameRect>0,0,1920,1080</FrameRect>
	</VideoTrackGroup>
	<AudioTrackGroup ObjectID="61" ClassID="test" Version="6">
		<TrackGroup Version="1"><Tracks Version="1"><Track Index="0" ObjectURef="at1"/></Tracks><FrameRate>5292000</FrameRate></TrackGroup>
	</AudioTrackGroup>
	<DataTrackGroup ObjectID="62" ClassID="test" Version="1">
		<TrackGroup Version="1"><Tracks Version="1"><Track Index="0" ObjectURef="dt1"/></Tracks></TrackGroup>
	</DataTrackGroup>
	<CaptionDataClipTrack ObjectUID="dt1" ClassID="test" Version="1">
		<ClipTrack Version="2"><ClipItems Version="3"><TrackItems Version="1"><TrackItem Index="0" ObjectRef="300"/></TrackItems></ClipItems></ClipTrack>
	</CaptionDataClipTrack>
	<CaptionDataClipTrackItem ObjectID="300" ClassID="test" Version="1"/>
	<VideoClipTrack ObjectUID="vt1" ClassID="test" Version="1">
		<ClipTrack Version="2">
			<Track Version="3"><IsLocked>true</IsLocked><IsMuted>false</IsMuted></Track>
			<ClipItems Version="3"><TrackItems Version="1"><TrackItem Index="0" ObjectRef="100"/><TrackItem Index="1" ObjectRef="101"/></TrackItems></ClipItems>
			<TransitionItems Version="3"><TrackItems Version="1"><TrackItem Index="0" ObjectRef="110"/></TrackItems></TransitionItems>
		</ClipTrack>
	</VideoClipTrack>
	<VideoClipTrackItem ObjectID="100" ClassID="test" Version="6">
		<ClipTrackItem Version="8">
			<ComponentOwner Version="1"><Components ObjectRef="120"/></ComponentOwner>
			<TrackItem Version="3"><Start>0</Start><End>508032000000</End></TrackItem>
			<SubClip ObjectRef="102"/>
		</ClipTrackItem>
	</VideoClipTrackItem>
	<SubClip ObjectID="102" ClassID="test" Version="5"><Clip ObjectRef="103"/><MasterClip ObjectURef="mc-shot"/><Name>shot A</Name></SubClip>
	<VideoClip ObjectID="103" ClassID="test" Version="11">
		<Clip Version="18">
			<Node Version="1"><Properties Version="1"><asl.clip.label.name>BE.Prefs.LabelColors.9</asl.clip.label.name></Properties></Node>
			<Source ObjectRef="12"/><InPoint>254016000000</InPoint><OutPoint>762048000000</OutPoint>
		</Clip>
	</VideoClip>
	<VideoComponentChain ObjectID="120" ClassID="test" Version="2">
		<ComponentChain Version="2"><Components Version="1">
			<Component Index="0" ObjectRef="121"/><Component Index="1" ObjectRef="122"/><Component Index="2" ObjectRef="123"/>
		</Components></ComponentChain>
	</VideoComponentChain>
	<VideoFilterComponent ObjectID="121" ClassID="test" Version="8">
		<Component Version="5"><Params Version="1"><Param Index="0" ObjectRef="130"/><Param Index="1" ObjectRef="131"/></Params><DisplayName>Motion</DisplayName><Bypass>false</Bypass><Intrinsic>true</Intrinsic></Component>
		<MatchName>AE.ADBE Motion</MatchName>
	</VideoFilterComponent>
	<PointComponentParam ObjectID="130" ClassID="test" Version="3"><Name>Position</Name><StartKeyframe>-91445760000000000,0.75:0.5,0,0,0,0,0,0,5,4,0,0,0,0</StartKeyframe><ParameterID>1</ParameterID></PointComponentParam>
	<VideoComponentParam ObjectID="131" ClassID="test" Version="9"><Name>Scale</Name><StartKeyframe>-91445760000000000,50.,0,0,0,0,0,0</StartKeyframe><ParameterID>2</ParameterID></VideoComponentParam>
	<VideoFilterComponent ObjectID="122" ClassID="test" Version="8">
		<Component Version="5"><Params Version="1"><Param Index="0" ObjectRef="132"/></Params><DisplayName>Opacity</DisplayName></Component>
		<MatchName>AE.ADBE Opacity</MatchName>
	</VideoFilterComponent>
	<VideoComponentParam ObjectID="132" ClassID="test" Version="9">
		<Name>Opacity</Name>
		<StartKeyframe>-91445760000000000,100.,0,0,0,0,0,0</StartKeyframe>
		<Keyframes>254016000000,100.,0,0,0,0,0,0;508032000000,25.,0,0,0,0,0,0;</Keyframes>
		<ParameterID>1</ParameterID>
	</VideoComponentParam>
	<VideoFilterComponent ObjectID="123" ClassID="test" Version="8">
		<Component Version="5"><DisplayName>Fancy Thing</DisplayName></Component>
		<MatchName>TEST.Fancy Thing</MatchName>
	</VideoFilterComponent>
	<VideoClipTrackItem ObjectID="101" ClassID="test" Version="6">
		<ClipTrackItem Version="8">
			<TrackItem Version="3"><Start>508032000000</Start><End>1016064000000</End></TrackItem>
			<SubClip ObjectRef="104"/>
			<IsMuted>true</IsMuted>
		</ClipTrackItem>
	</VideoClipTrackItem>
	<SubClip ObjectID="104" ClassID="test" Version="5"><Clip ObjectRef="105"/><MasterClip ObjectURef="mc-shot"/><Name>shot B</Name></SubClip>
	<VideoClip ObjectID="105" ClassID="test" Version="11"><Clip Version="18"><Source ObjectRef="12"/><PlaybackSpeed>0.5</PlaybackSpeed><InPoint>0</InPoint></Clip></VideoClip>
	<VideoTransitionTrackItem ObjectID="110" ClassID="test" Version="4">
		<TransitionTrackItem Version="3">
			<TrackItem Version="4"><Start>381024000000</Start><End>635040000000</End></TrackItem>
			<DisplayName>Cross Dissolve</DisplayName>
			<HasOutgoingClip>true</HasOutgoingClip>
			<HasIncomingClip>true</HasIncomingClip>
			<MatchName>TEST.Cross Dissolve</MatchName>
			<Alignment>127008000000</Alignment>
		</TransitionTrackItem>
	</VideoTransitionTrackItem>
	<AudioClipTrack ObjectUID="at1" ClassID="test" Version="6">
		<ClipTrack Version="2">
			<Track Version="3"><IsMuted>false</IsMuted></Track>
			<ClipItems Version="3"><TrackItems Version="1"><TrackItem Index="0" ObjectRef="200"/></TrackItems></ClipItems>
		</ClipTrack>
	</AudioClipTrack>
	<AudioClipTrackItem ObjectID="200" ClassID="test" Version="6">
		<ClipTrackItem Version="8">
			<ComponentOwner Version="1"><Components ObjectRef="220"/></ComponentOwner>
			<TrackItem Version="3"><Start>0</Start><End>508032000000</End></TrackItem>
			<SubClip ObjectRef="202"/>
		</ClipTrackItem>
	</AudioClipTrackItem>
	<SubClip ObjectID="202" ClassID="test" Version="5"><Clip ObjectRef="203"/><MasterClip ObjectURef="mc-shot"/><Name>shot A</Name></SubClip>
	<AudioClip ObjectID="203" ClassID="test" Version="8"><Clip Version="18"><Source ObjectRef="13"/><InPoint>254016000000</InPoint></Clip><Gain>2</Gain></AudioClip>
	<AudioComponentChain ObjectID="220" ClassID="test" Version="1">
		<ComponentChain Version="3"><Components Version="1"><Component Index="0" ObjectRef="221"/></Components></ComponentChain>
	</AudioComponentChain>
	<AudioFilterComponent ObjectID="221" ClassID="test" Version="1">
		<AudioComponent Version="3"><Component Version="7"><Params Version="1"><Param Index="0" ObjectRef="230"/><Param Index="1" ObjectRef="231"/></Params><Intrinsic>true</Intrinsic></Component></AudioComponent>
		<FilterMatchName>Internal Volume Stereo</FilterMatchName>
	</AudioFilterComponent>
	<AudioComponentParam ObjectID="230" ClassID="test" Version="9"><Name>Mute</Name></AudioComponentParam>
	<AudioComponentParam ObjectID="231" ClassID="test" Version="9">
		<Name>Level</Name>
		<StartKeyframe>-91445760000000000,0.177827941,0,0,0,0,0,0</StartKeyframe>
		<Keyframes>254016000000,0.177827941,0,0,0,0,0,0;508032000000,1.,0,0,0,0,0,0;</Keyframes>
	</AudioComponentParam>
</PremiereData>
"#;

/// A sequence whose only clip plays the sequence itself.
const CYCLE: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<PremiereData Version="3">
	<Project ObjectRef="1"/>
	<Project ObjectID="1" ClassID="test" Version="45"/>
	<Sequence ObjectUID="seq-a" ClassID="test" Version="11">
		<TrackGroups Version="1"><TrackGroup Version="1" Index="0"><First>video</First><Second ObjectRef="2"/></TrackGroup></TrackGroups>
		<Name>Loop</Name>
	</Sequence>
	<VideoTrackGroup ObjectID="2" ClassID="test" Version="11">
		<TrackGroup Version="1"><Tracks Version="1"><Track Index="0" ObjectURef="vt"/></Tracks><FrameRate>10160640000</FrameRate></TrackGroup>
	</VideoTrackGroup>
	<VideoClipTrack ObjectUID="vt" ClassID="test" Version="1">
		<ClipTrack Version="2"><ClipItems Version="3"><TrackItems Version="1"><TrackItem Index="0" ObjectRef="3"/></TrackItems></ClipItems></ClipTrack>
	</VideoClipTrack>
	<VideoClipTrackItem ObjectID="3" ClassID="test" Version="6">
		<ClipTrackItem Version="8"><TrackItem Version="3"><Start>0</Start><End>254016000000</End></TrackItem><SubClip ObjectRef="4"/></ClipTrackItem>
	</VideoClipTrackItem>
	<SubClip ObjectID="4" ClassID="test" Version="5"><Clip ObjectRef="5"/><MasterClip ObjectURef="mc"/><Name>Loop</Name></SubClip>
	<MasterClip ObjectUID="mc" ClassID="test" Version="10"><Clips Version="1"><Clip Index="0" ObjectRef="5"/></Clips></MasterClip>
	<VideoClip ObjectID="5" ClassID="test" Version="11"><Clip Version="18"><Source ObjectRef="6"/></Clip></VideoClip>
	<VideoSequenceSource ObjectID="6" ClassID="test" Version="3"><SequenceSource Version="4"><Sequence ObjectURef="seq-a"/></SequenceSource></VideoSequenceSource>
</PremiereData>
"#;

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::default());
    enc.write_all(bytes).unwrap();
    enc.finish().unwrap();
    out
}

fn imported(bytes: &[u8]) -> (Imported, Report) {
    import(bytes, Format::Prproj, Some("/proj")).unwrap()
}

#[test]
fn detects_plain_and_gzipped_projects() {
    assert_eq!(detect(PROJECT.as_bytes(), Some("prproj")), Some(Format::Prproj));
    // Premiere also writes the XML uncompressed; the content decides, not the extension
    assert_eq!(detect(PROJECT.as_bytes(), Some("xml")), Some(Format::Prproj));
    assert_eq!(detect(&gzip(PROJECT.as_bytes()), Some("prproj")), Some(Format::Prproj));
    assert_eq!(detect(&gzip(PROJECT.as_bytes()), None), Some(Format::Prproj));
    assert_eq!(Format::from_extension(".PRPROJ"), Some(Format::Prproj));
    assert!(!Format::ALL.contains(&Format::Prproj), "import only");
    let (a, _) = imported(PROJECT.as_bytes());
    let (b, _) = imported(&gzip(PROJECT.as_bytes()));
    assert_eq!(a.project.items.len(), b.project.items.len());
}

#[test]
fn imports_sequences_tracks_clips_and_bins() {
    let (imp, report) = imported(PROJECT.as_bytes());
    let p = &imp.project;
    assert_eq!(imp.sequences.len(), 1);
    let seq_item = p.item(imp.sequences[0]).unwrap();
    assert_eq!(seq_item.name, "Main");
    let seq = seq_item.as_sequence().unwrap();
    assert_eq!(seq.settings.frame_rate, FrameRate::FPS_25);
    assert_eq!((seq.settings.width, seq.settings.height, seq.settings.sample_rate), (1920, 1080, 48_000));

    // bins and labels
    let footage = p.root.children.iter().find_map(|c| match c {
        BinEntry::Bin(b) if b.name == "Footage" => Some(b),
        _ => None,
    });
    let footage = footage.expect("Footage bin");
    let shot = footage
        .children
        .iter()
        .filter_map(|c| match c {
            BinEntry::Item(i) => p.item(*i),
            BinEntry::Bin(_) => None,
        })
        .find(|i| i.name == "shot.mov")
        .expect("shot.mov in Footage");
    assert_eq!(shot.label, Label::Forest);
    let media = shot.as_media().unwrap();
    assert!(matches!(&media.media, redrob_film_project::MediaRef::File { path } if path == "/media/shot.mov"));
    let v = media.info.video.as_ref().unwrap();
    assert_eq!((v.width, v.height, v.frame_rate), (1920, 1080, FrameRate::FPS_25));
    assert_eq!(media.info.audio_streams.first().map(|a| (a.sample_rate, a.channels)), Some((48_000, 2)));
    assert_eq!(media.markers.len(), 1);
    assert_eq!(media.markers[0].name, "take 2");
    assert_eq!(media.markers[0].start, Tick(S));

    // offline media and graphics are kept as offline items
    let offline = p.items.values().find(|i| i.name == "lost.mp4").and_then(|i| i.as_media()).expect("lost.mp4");
    assert!(offline.offline);
    let gfx = p.items.values().find(|i| i.name == "Graphic").and_then(|i| i.as_media()).expect("Graphic");
    assert!(gfx.offline);

    // video track: locked, two clips, the second disabled and at half speed
    let v1 = &seq.video_tracks[0];
    assert!(v1.locked);
    assert_eq!(v1.items.len(), 2);
    let (a, b) = (&v1.items[0], &v1.items[1]);
    assert_eq!((a.name.as_str(), a.start, a.duration, a.source_in), ("shot A", Tick(0), Tick(2 * S), Tick(S)));
    assert_eq!(a.label, Label::Blue);
    assert!(a.enabled);
    assert_eq!((b.start, b.duration, b.speed, b.enabled, b.reverse), (Tick(2 * S), Tick(2 * S), 0.5, false, false));

    // cross dissolve centred on the cut
    assert_eq!(v1.transitions.len(), 1);
    let t = &v1.transitions[0];
    assert_eq!((t.start, t.duration, t.from, t.to, t.align), (Tick(3 * S / 2), Tick(S), Some(a.id), Some(b.id), TransitionAlign::CenterAtCut));
    assert_eq!(t.effect.effect, "cross_dissolve");

    // Motion: position is a fraction of the sequence frame; Opacity animated in source time
    let pos = a.effect("motion").and_then(|e| e.param("position")).and_then(|p| p.value.as_vec2()).unwrap();
    assert!((pos.x - 1440.0).abs() < 1e-9 && (pos.y - 540.0).abs() < 1e-9, "{pos:?}");
    assert_eq!(a.effect("motion").and_then(|e| e.param("scale")).and_then(|p| p.value.as_f64()), Some(50.0));
    let op = a.effect("opacity").and_then(|e| e.param("opacity")).unwrap();
    assert_eq!(op.keyframes.len(), 2);
    assert_eq!(op.keyframes[0].time, Tick(S));
    assert_eq!(op.keyframes[1].value.as_f64(), Some(25.0));

    // audio: linked to the first video clip, clip gain, Volume keyframes (1.0 = +15 dB)
    let a1 = &seq.audio_tracks[0].items[0];
    assert!(a1.link.is_some());
    assert_eq!(a1.link, a.link);
    assert!((a1.gain_db - 6.0206).abs() < 1e-3, "{}", a1.gain_db);
    let lvl = a1.effect("volume").and_then(|e| e.param("level")).unwrap();
    assert_eq!(lvl.keyframes.len(), 2);
    assert!(lvl.keyframes[0].value.as_f64().is_some_and(|v| v.abs() < 1e-3));
    assert!(lvl.keyframes[1].value.as_f64().is_some_and(|v| (v - 15.0).abs() < 1e-9));

    // sequence marker
    assert_eq!(seq.markers.len(), 1);
    let m = &seq.markers[0];
    assert_eq!((m.name.as_str(), m.comment.as_str(), m.start, m.duration, m.kind), ("Hello", "note", Tick(S), Tick(S / 2), MarkerKind::Chapter));

    // what did not carry over is named
    assert!(report.mentions("Fancy Thing"), "{report}");
    assert!(report.mentions("caption"), "{report}");
    assert!(report.mentions("Graphic"), "{report}");
}

#[test]
fn a_sequence_that_contains_itself_is_reported_not_followed() {
    let (imp, report) = imported(CYCLE.as_bytes());
    assert_eq!(imp.sequences.len(), 1);
    assert!(report.mentions("contains itself"), "{report}");
}

#[test]
fn hostile_documents_never_panic() {
    assert!(import(b"", Format::Prproj, None).is_err());
    assert!(import(b"<PremiereData", Format::Prproj, None).is_err());
    assert!(import(b"<Other/>", Format::Prproj, None).is_err());
    assert!(import(&[0x1f, 0x8b, 0x08, 0, 0, 0], Format::Prproj, None).is_err());
    // references to nothing, self references, absurd numbers
    let odd = PROJECT
        .replace("ObjectRef=\"12\"", "ObjectRef=\"999\"")
        .replace("<End>1016064000000</End>", "<End>9223372036854775807</End>")
        .replace("<FrameRate>10160640000</FrameRate>", "<FrameRate>0</FrameRate>")
        .replace("0,0,1920,1080", "5,5,-1,NaN")
        .replace("<PlaybackSpeed>0.5</PlaybackSpeed>", "<PlaybackSpeed>NaN</PlaybackSpeed>");
    let r = catch_unwind(|| import(odd.as_bytes(), Format::Prproj, None));
    assert!(r.is_ok(), "panicked on odd values");

    // mutation fuzz: truncations and byte flips of the plain and the gzipped document
    let plain = PROJECT.as_bytes().to_vec();
    let packed = gzip(&plain);
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for i in 0..400 {
        let src = if i % 2 == 0 { &plain } else { &packed };
        let mut m = src.clone();
        match next() % 3 {
            0 => m.truncate((next() as usize) % src.len()),
            1 => {
                for _ in 0..1 + next() % 8 {
                    let at = (next() as usize) % m.len();
                    m[at] ^= 1 << (next() % 8);
                }
            }
            _ => {
                let at = (next() as usize) % m.len();
                let len = ((next() as usize) % 64).min(m.len() - at);
                let chunk = m[at..at + len].to_vec();
                let tail = m.split_off(at);
                m.extend_from_slice(&chunk);
                m.extend_from_slice(&tail);
            }
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            let _ = detect(&m, Some("prproj"));
            let _ = import(&m, Format::Prproj, Some("/proj"));
        }));
        assert!(r.is_ok(), "panic on mutation {i}");
    }
}

/// Opt-in: every project in `REDROB_FILM_PRPROJ_DIR` imports without a panic, with one sequence
/// per `<Sequence>` object and one clip per track item a clip track lists.
#[test]
fn real_projects_from_env_dir() {
    let Some(dir) = std::env::var_os("REDROB_FILM_PRPROJ_DIR") else { return };
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_file() {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_string);
        if detect(&bytes, ext.as_deref()) != Some(Format::Prproj) {
            continue;
        }
        let name = path.display().to_string();
        let text = prproj::decode_document(&bytes).unwrap();
        let doc = roxmltree::Document::parse_with_options(&text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).unwrap();
        let root = doc.root_element();
        let id = |n: roxmltree::Node| n.attribute("ObjectID").or_else(|| n.attribute("ObjectUID")).map(str::to_string);
        let expected_seqs = root.children().filter(|n| n.has_tag_name("Sequence") && id(*n).is_some()).count();
        let listed: HashSet<String> = root
            .descendants()
            .filter(|n| n.has_tag_name("ClipItems"))
            .flat_map(|c| c.descendants().filter(|n| n.has_tag_name("TrackItem")))
            .filter_map(|n| n.attribute("ObjectRef").or_else(|| n.attribute("ObjectURef")).map(str::to_string))
            .collect();
        let expected_items = root
            .children()
            .filter(|n| n.has_tag_name("VideoClipTrackItem") || n.has_tag_name("AudioClipTrackItem"))
            .filter(|n| id(*n).is_some_and(|i| listed.contains(&i)))
            .count();
        let r = catch_unwind(AssertUnwindSafe(|| import(&bytes, Format::Prproj, path.parent().and_then(|d| d.to_str()))));
        let r = r.unwrap_or_else(|_| panic!("{name}: panicked"));
        let (imp, report) = match r {
            Ok(x) => x,
            Err(e) if expected_seqs == 0 => {
                eprintln!("{name}: {e} (no sequences)");
                checked += 1;
                continue;
            }
            Err(e) => panic!("{name}: {e}"),
        };
        let seqs: Vec<_> = imp.project.items.values().filter_map(|i| i.as_sequence()).collect();
        let items: usize = seqs.iter().map(|s| s.all_tracks().map(|t| t.items.len()).sum::<usize>()).sum();
        assert_eq!(seqs.len(), expected_seqs, "{name}: sequences\n{report}");
        if expected_seqs > 0 {
            assert!(!imp.sequences.is_empty(), "{name}: no top-level sequence");
        }
        assert_eq!(items, expected_items, "{name}: track items\n{report}");
        // every media item lives in the fragment's bins or root
        assert!(imp.project.items.values().all(|i| !matches!(i.kind, ItemKind::Subclip { .. })));
        eprintln!("{name}: {} sequence(s), {items} clip(s), {} report line(s)", seqs.len(), report.entries.len());
        checked += 1;
    }
    assert!(checked > 0, "no Premiere project in {}", std::path::Path::new(&dir).display());
}
