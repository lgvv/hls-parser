//! timeline 계약: half-open 구간, PDT 추론, gap, property 검사

use hls_core::*;
use proptest::prelude::*;
use std::fmt::Write;

fn snapshot(body: &str) -> std::sync::Arc<MediaSnapshot> {
    parse(
        &format!("#EXTM3U\n#EXT-X-TARGETDURATION:10\n{body}"),
        &ParseOptions::default(),
    )
    .unwrap()
    .media()
    .unwrap()
    .clone()
}
fn target(value: LocateResult) -> SeekTarget {
    let LocateResult::Found(target) = value else {
        panic!("expected Found: {value:?}")
    };
    target
}

#[test]
fn relative_half_open_boundaries_gaps_and_snapshot_ownership() {
    let snapshot = snapshot("#EXTINF:2.5,\na\n#EXT-X-GAP\n#EXTINF:1,\nb\n#EXTINF:3,\nc\n");
    let timeline = RelativeTimeline::build(snapshot.clone()).unwrap();
    drop(snapshot);
    assert_eq!(
        target(timeline.locate(ElapsedTime::from_nanos(2_499_999_999))).segment_index(),
        0
    );
    assert_eq!(
        timeline.locate(ElapsedTime::from_nanos(2_500_000_000)),
        LocateResult::Gap {
            segment_index: Some(1)
        }
    );
    let found = target(timeline.locate(ElapsedTime::from_nanos(3_500_000_001)));
    assert_eq!(found.segment_index(), 2);
    assert_eq!(found.offset_in_segment().as_nanos(), 1);
    assert_eq!(timeline.duration().as_nanos(), 6_500_000_000);
    assert_eq!(
        timeline.locate(timeline.duration()),
        LocateResult::AfterWindow
    );
}

#[test]
fn pdt_backfills_and_infers_but_keeps_declared_values_distinct() {
    let snapshot = snapshot(
        "#EXTINF:2,\na\n#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:02Z\n#EXTINF:3,\nb\n#EXTINF:1,\nc\n",
    );
    let timeline = WallClockTimeline::build(snapshot).unwrap();
    assert_eq!(timeline.start().as_unix_nanos(), 0);
    assert_eq!(timeline.end().as_unix_nanos(), 6_000_000_000);
    assert_eq!(
        timeline.snapshot().segments()[0].declared_program_time(),
        None
    );
    assert_eq!(
        timeline.locate(UtcTimestamp::from_unix_nanos(-1)),
        LocateResult::BeforeWindow
    );
    assert_eq!(
        target(timeline.locate(UtcTimestamp::from_unix_nanos(2_000_000_000))).segment_index(),
        1
    );
    assert_eq!(timeline.locate(timeline.end()), LocateResult::AfterWindow);
}

#[test]
fn discontinuity_needs_its_own_anchor_and_can_create_a_wall_clock_gap() {
    let missing = snapshot(
        "#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00Z\n#EXTINF:1,\na\n#EXT-X-DISCONTINUITY\n#EXTINF:1,\nb\n",
    );
    assert!(RelativeTimeline::build(missing.clone()).is_ok());
    assert_eq!(
        WallClockTimeline::build(missing).unwrap_err(),
        TimelineError::MissingTimeMapping { segment_index: 1 }
    );
    let mapped = snapshot(
        "#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00Z\n#EXTINF:1,\na\n#EXT-X-DISCONTINUITY\n#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:03Z\n#EXTINF:1,\nb\n",
    );
    let timeline = WallClockTimeline::build(mapped).unwrap();
    assert_eq!(
        timeline.locate(UtcTimestamp::from_unix_nanos(1_000_000_000)),
        LocateResult::Gap {
            segment_index: None
        }
    );
    assert_eq!(
        target(timeline.locate(UtcTimestamp::from_unix_nanos(3_000_000_000))).segment_index(),
        1
    );
}

#[test]
fn conflicting_anchors_and_overlapping_runs_fail_construction() {
    for discontinuity in ["", "#EXT-X-DISCONTINUITY\n"] {
        let bad = snapshot(&format!(
            "#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00Z\n#EXTINF:2,\na\n{discontinuity}#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:01Z\n#EXTINF:1,\nb\n"
        ));
        assert_eq!(
            WallClockTimeline::build(bad).unwrap_err(),
            TimelineError::ConflictingTimeMapping { segment_index: 1 }
        );
    }
    // timeline은 허용 오차를 받지 않음, 1나노초라도 어긋나면 실패
    // 오차를 허용하려면 tracker의 pdt_tolerance_nanos처럼 명시적으로 정해야 함
    let drift = snapshot(
        "#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00Z\n#EXTINF:1,\na\n#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:01.000000001Z\n#EXTINF:1,\nb\n",
    );
    assert!(matches!(
        WallClockTimeline::build(drift),
        Err(TimelineError::ConflictingTimeMapping { .. })
    ));
}

#[test]
fn empty_missing_time_and_signed_timestamp_overflow() {
    let empty = snapshot("");
    assert_eq!(
        RelativeTimeline::build(empty).unwrap_err(),
        TimelineError::EmptySnapshot
    );
    assert_eq!(
        WallClockTimeline::build(snapshot("#EXTINF:1,\na\n")).unwrap_err(),
        TimelineError::MissingTimeMapping { segment_index: 0 }
    );
    let overflowing =
        snapshot("#EXT-X-PROGRAM-DATE-TIME:2262-04-11T23:47:16.854775807Z\n#EXTINF:1,\na\n");
    assert_eq!(
        WallClockTimeline::build(overflowing).unwrap_err(),
        TimelineError::Overflow { segment_index: 0 }
    );
}

proptest! {
    #[test]
    fn binary_search_matches_linear_oracle(
        entries in prop::collection::vec((1u32..10_000_000, any::<bool>()), 1..80),
        queries in prop::collection::vec(any::<u64>(), 1..100)
    ) {
        let mut text = String::from("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:100\n#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00Z\n");
        let mut total = 0u64;
        for (i, (duration, gap)) in entries.iter().enumerate() {
            if *gap { text.push_str("#EXT-X-GAP\n"); }
            write!(text, "#EXTINF:0.{duration:09},\n{i}.ts\n").unwrap();
            total += u64::from(*duration);
        }
        let snapshot = parse(&text, &ParseOptions::default()).unwrap().media().unwrap().clone();
        let relative = RelativeTimeline::build(snapshot.clone()).unwrap();
        let wall = WallClockTimeline::build(snapshot).unwrap();
        let mut probes: Vec<u64> = queries.iter().map(|q| q % (total+1)).collect();
        probes.extend([0, total, total-1]);
        let mut cursor = 0;
        for (d, _) in &entries { probes.push(cursor); cursor += u64::from(*d); }
        for probe in probes {
            let actual = relative.locate(ElapsedTime::from_nanos(probe));
            prop_assert_eq!(actual, wall.locate(UtcTimestamp::from_unix_nanos(i64::try_from(probe).unwrap())));
            if probe >= total { prop_assert_eq!(actual, LocateResult::AfterWindow); continue; }
            let mut cursor = 0;
            for (i, (d, gap)) in entries.iter().enumerate() {
                let end = cursor + u64::from(*d);
                if probe < end {
                    if *gap { prop_assert_eq!(actual, LocateResult::Gap { segment_index: Some(i) }); }
                    else {
                        let t = target(actual);
                        prop_assert_eq!(t.segment_index(), i);
                        prop_assert_eq!(t.media_sequence(), 100+i as u64);
                        prop_assert_eq!(t.offset_in_segment().as_nanos(), probe-cursor);
                    }
                    break;
                }
                cursor = end;
            }
        }
    }

    #[test]
    fn arbitrary_text_never_panics(text in "(?s).{0,2048}") {
        let _ = parse(&text, &ParseOptions::default());
        let _ = parse(&format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n{text}"), &ParseOptions::default());
    }
}
