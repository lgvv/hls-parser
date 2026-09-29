//! "이 시각은 어느 segment의 어디인가"에 답하는 인덱스
//! segment 길이를 누적해 구간 배열을 한 번 만들고, 조회는 그 배열의 이진 탐색
//! snapshot을 Arc로 잡고 있어 문서를 버린 뒤에도 조회 가능

use crate::{ElapsedTime, MediaSnapshot, Segment, UtcTimestamp};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
/// timeline 생성 실패 이유, 만들어진 timeline의 조회는 실패 없음
pub enum TimelineError {
    /// segment가 없는 snapshot
    #[error("cannot index an empty snapshot")]
    EmptySnapshot,
    /// `EXT-X-PROGRAM-DATE-TIME` anchor가 없는 discontinuity run
    #[error("missing program time in discontinuity run starting at segment {segment_index}")]
    MissingTimeMapping {
        /// run의 첫 segment
        segment_index: usize,
    },
    /// `EXTINF` 누적과 어긋나는 명시적 anchor, 또는 겹치는 run
    #[error("conflicting program time at segment {segment_index}")]
    ConflictingTimeMapping {
        /// 충돌을 감지한 segment
        segment_index: usize,
    },
    /// 시간 의미를 구현하지 않은 태그가 있는 snapshot
    #[error("timeline semantics not supported for {tag}")]
    UnsupportedTimingFeature {
        /// 막은 태그 이름
        tag: String,
    },
    /// 표현 범위를 벗어난 누적 시간
    #[error("time overflow at segment {segment_index}")]
    Overflow {
        /// 누적이 오버플로한 segment
        segment_index: usize,
    },
}

/// 조회한 timeline이 잡고 있는 불변 snapshot 안의 위치
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeekTarget {
    segment_index: usize,
    media_sequence: u64,
    offset: ElapsedTime,
}
impl SeekTarget {
    pub(crate) const fn new(
        segment_index: usize,
        media_sequence: u64,
        offset: ElapsedTime,
    ) -> Self {
        Self {
            segment_index,
            media_sequence,
            offset,
        }
    }
    /// 플랫폼 경계를 넘어온 target 복원
    /// 값은 받는 timeline이나 tracker가 다시 검증, 신뢰할 수 있는 위치를 만드는 것은 아님
    #[doc(hidden)]
    pub const fn new_for_ffi(
        segment_index: usize,
        media_sequence: u64,
        offset: ElapsedTime,
    ) -> Self {
        Self::new(segment_index, media_sequence, offset)
    }
    /// snapshot의 segment 목록에서 0부터 세는 인덱스
    pub fn segment_index(self) -> usize {
        self.segment_index
    }
    /// 해당 segment의 media sequence 번호, 갱신된 playlist와의 대조용
    pub fn media_sequence(self) -> u64 {
        self.media_sequence
    }
    /// segment 시작부터의 시간, 항상 segment 길이 미만
    pub fn offset_in_segment(self) -> ElapsedTime {
        self.offset
    }
}

/// timeline 조회 결과
/// 모든 구간은 `[start, end)`라 segment의 end 시각은 다음 segment 소속, 마지막 end는 `AfterWindow`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocateResult {
    /// 재생 가능한 segment 안의 시각
    Found(SeekTarget),
    /// 재생할 것이 없는 시각
    Gap {
        /// `Some(index)`는 명시적 `EXT-X-GAP`, `None`은 discontinuity run 사이의 매핑되지 않은 구간
        segment_index: Option<usize>,
    },
    /// 첫 segment보다 이른 시각
    BeforeWindow,
    /// 마지막 segment의 끝 또는 그 이후 시각
    AfterWindow,
}

/// 구간 `[start, end)`, T는 상대 시간이면 u64, UTC면 i64
#[derive(Debug, Clone, Copy)]
pub(crate) struct Interval<T> {
    pub(crate) start: T,
    pub(crate) end: T,
}

/// snapshot의 첫 segment부터의 경과 시간을 키로 하는 인덱스
#[derive(Debug, Clone)]
pub struct RelativeTimeline {
    snapshot: Arc<MediaSnapshot>,
    index: Vec<Interval<u64>>,
}
impl RelativeTimeline {
    /// 오버플로를 검사하며 `EXTINF` 길이 누적
    pub fn build(snapshot: Arc<MediaSnapshot>) -> Result<Self, TimelineError> {
        validate_snapshot(&snapshot)?;
        let index = relative_index(snapshot.segments())?;
        Ok(Self { snapshot, index })
    }
    /// 색인한 snapshot
    pub fn snapshot(&self) -> &Arc<MediaSnapshot> {
        &self.snapshot
    }
    /// 모든 segment 길이의 합
    pub fn duration(&self) -> ElapsedTime {
        ElapsedTime::from_nanos(self.index.last().map_or(0, |i| i.end))
    }
    /// `time`을 포함하는 segment의 이진 탐색
    pub fn locate(&self, time: ElapsedTime) -> LocateResult {
        locate(self.snapshot.segments(), &self.index, time.as_nanos())
    }
}

/// `EXT-X-PROGRAM-DATE-TIME` anchor에서 유도한 UTC 키 인덱스
#[derive(Debug, Clone)]
pub struct WallClockTimeline {
    snapshot: Arc<MediaSnapshot>,
    index: Vec<Interval<i64>>,
}
impl WallClockTimeline {
    /// 각 discontinuity run 안에서 PDT anchor를 앞뒤로 전파
    /// 추론은 discontinuity를 넘지 않음, 모든 run에 anchor 필수
    /// run 안의 모든 명시적 anchor는 EXTINF 누적과 정확히 일치해야 함
    pub fn build(snapshot: Arc<MediaSnapshot>) -> Result<Self, TimelineError> {
        validate_snapshot(&snapshot)?;
        let segments = snapshot.segments();
        let relative = relative_index(segments)?;
        let index = wall_clock_index(
            segments,
            &relative,
            |i| segments[i].discontinuity(),
            |i| {
                segments[i]
                    .declared_program_time()
                    .map(UtcTimestamp::as_unix_nanos)
            },
            0,
        )?;
        Ok(Self { snapshot, index })
    }
    /// 색인한 snapshot
    pub fn snapshot(&self) -> &Arc<MediaSnapshot> {
        &self.snapshot
    }
    /// 첫 segment의 시작, `build`가 항목 하나 이상을 보장
    pub fn start(&self) -> UtcTimestamp {
        UtcTimestamp::from_unix_nanos(self.index[0].start)
    }
    /// 마지막 segment의 종료(exclusive)
    pub fn end(&self) -> UtcTimestamp {
        UtcTimestamp::from_unix_nanos(self.index[self.index.len() - 1].end)
    }
    /// `time`을 포함하는 segment의 이진 탐색
    pub fn locate(&self, time: UtcTimestamp) -> LocateResult {
        locate(self.snapshot.segments(), &self.index, time.as_unix_nanos())
    }
}

fn validate_snapshot(snapshot: &MediaSnapshot) -> Result<(), TimelineError> {
    if let Some(tag) = snapshot.timeline_blockers().first() {
        return Err(TimelineError::UnsupportedTimingFeature { tag: tag.clone() });
    }
    if snapshot.segment_count() == 0 {
        return Err(TimelineError::EmptySnapshot);
    }
    Ok(())
}

/// 오버플로를 검사한 `EXTINF` 누적 구간
pub(crate) fn relative_index(segments: &[Segment]) -> Result<Vec<Interval<u64>>, TimelineError> {
    let mut index = Vec::with_capacity(segments.len());
    let mut start = 0u64;
    for (i, segment) in segments.iter().enumerate() {
        let end = start
            .checked_add(segment.duration().as_nanos())
            .ok_or(TimelineError::Overflow { segment_index: i })?;
        index.push(Interval { start, end });
        start = end;
    }
    Ok(index)
}

/// 각 run 안에서 anchor를 앞뒤로 전파
/// run은 인덱스 0과 `starts_run`이 참인 곳에서 시작, 추론은 run 경계를 넘지 않음
/// 모든 run에 anchor 필수, `anchor_of`가 Unix 나노초로 제공
/// run 안의 anchor는 `EXTINF` 누적과 `tolerance` 나노초 이내로 일치해야 하고 run끼리 겹치면 안 됨
pub(crate) fn wall_clock_index(
    segments: &[Segment],
    relative: &[Interval<u64>],
    starts_run: impl Fn(usize) -> bool,
    anchor_of: impl Fn(usize) -> Option<i64>,
    tolerance: u64,
) -> Result<Vec<Interval<i64>>, TimelineError> {
    let mut index: Vec<Interval<i64>> = Vec::with_capacity(segments.len());
    let mut first = 0;
    while first < segments.len() {
        let last = (first + 1..segments.len())
            .find(|&i| starts_run(i))
            .unwrap_or(segments.len());
        let (anchor, anchor_time) = (first..last)
            .find_map(|i| anchor_of(i).map(|time| (i, time)))
            .ok_or(TimelineError::MissingTimeMapping {
                segment_index: first,
            })?;
        let base = i128::from(anchor_time) - i128::from(relative[anchor].start);
        for (i, interval) in relative.iter().enumerate().take(last).skip(first) {
            let start = i64::try_from(base + i128::from(interval.start))
                .map_err(|_| TimelineError::Overflow { segment_index: i })?;
            let end = i64::try_from(base + i128::from(interval.end))
                .map_err(|_| TimelineError::Overflow { segment_index: i })?;
            let disagrees = anchor_of(i).is_some_and(|declared| {
                (i128::from(declared) - i128::from(start)).unsigned_abs() > u128::from(tolerance)
            });
            if disagrees || index.last().is_some_and(|previous| previous.end > start) {
                return Err(TimelineError::ConflictingTimeMapping { segment_index: i });
            }
            index.push(Interval { start, end });
        }
        first = last;
    }
    Ok(index)
}

pub(crate) fn locate<T: Copy + Ord + Into<i128>>(
    segments: &[Segment],
    index: &[Interval<T>],
    time: T,
) -> LocateResult {
    if time < index[0].start {
        return LocateResult::BeforeWindow;
    }
    if time >= index[index.len() - 1].end {
        return LocateResult::AfterWindow;
    }
    let position = index.partition_point(|entry| entry.start <= time) - 1;
    let segment = &segments[position];
    if time >= index[position].end {
        return LocateResult::Gap {
            segment_index: None,
        };
    }
    if segment.gap() {
        return LocateResult::Gap {
            segment_index: Some(position),
        };
    }
    // 위에서 [start, end) 포함을 확인했고 모든 구간 길이는 u64 segment 길이
    // 그래서 차이는 음수가 아니고 u64에 들어감
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "위에서 검사한 segment 길이가 상한"
    )]
    let offset = (time.into() - index[position].start.into()) as u64;
    LocateResult::Found(SeekTarget {
        segment_index: position,
        media_sequence: segment.media_sequence(),
        offset: ElapsedTime::from_nanos(offset),
    })
}
