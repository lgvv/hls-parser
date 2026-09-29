//! 타임 시프트를 위한 모듈
//!
//! 라이브 playlist는 갱신마다 최근 segment 몇 개만 보여줌, 앞은 잘리고 뒤에 새것이 붙음
//! 최신 응답만 들고 있으면 이미 잘려 나간 segment는 찾을 수 없으므로,
//! 받은 갱신을 media sequence 기준으로 모두 이어 붙여 하나의 이력으로 유지
//! 그러면 "10분 전"처럼 창 밖으로 밀려난 시각도 답할 수 있음
//! 각 갱신은 `EXT-X-PROGRAM-DATE-TIME`을 첫 segment에만 실어 오는 경우가 많아 anchor 처리가 핵심
//! 네트워킹 없음, 호출자가 playlist를 받아 파싱한 snapshot을 넘김

use crate::{
    domain::{MediaMetadata, MediaSnapshot, Segment},
    time::{ElapsedTime, UtcTimestamp},
    timeline::{
        Interval, LocateResult, SeekTarget, TimelineError, locate, relative_index, wall_clock_index,
    },
};

/// [`PlaylistTracker`]의 제한과 정책
#[derive(Debug, Clone)]
pub struct TrackerOptions {
    /// 이 개수를 넘으면 가장 오래된 segment부터 퇴출
    pub max_segments: usize,
    /// 명시적 `EXT-X-PROGRAM-DATE-TIME`이 run anchor 기준 `EXTINF` 누적과 어긋나도 되는 한도
    /// 인코더 시계와 EXTINF 누적은 스트림마다 다르게 어긋나므로 값은 실측 후 정하는 것이 맞음
    pub pdt_tolerance_nanos: u64,
}
impl Default for TrackerOptions {
    fn default() -> Self {
        Self {
            max_segments: 100_000,
            // 어긋남을 재기 전에는 엄격하게, 조용히 틀린 시각을 주는 것보다 거절이 낫다는 판단
            pdt_tolerance_nanos: 0,
        }
    }
}

/// 갱신 거절 이유, 오류 후 tracker는 그대로
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TrackerError {
    /// 추적 중인 sequence에 다른 미디어를 실은 segment
    /// 또는 시간이 정해진 이력과 어긋나는 program time을 실은 갱신
    #[error("segment {media_sequence} contradicts the tracked history")]
    ConflictingSegment {
        /// 충돌한 media sequence 번호
        media_sequence: u64,
    },
    /// 시간 의미를 구현하지 않은 태그가 있는 snapshot
    #[error("tracking not supported for {tag}")]
    UnsupportedTimingFeature {
        /// 막은 태그 이름
        tag: String,
    },
}

/// 갱신 한 번이 바꾼 내용
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// 추가하거나 채운 segment 수
    pub added: usize,
    /// `max_segments`를 지키려고 버린 가장 오래된 segment 수
    pub evicted: usize,
    /// 이력에 이미 있는 내용만 담은 갱신인지 여부
    pub stale: bool,
    /// 이력 전체에 UTC 인덱스가 있으면 `None`, 아니면 anchor가 아직 없는 run이 있음
    /// 모순은 여기 보고되지 않고 `apply`가 거절
    pub timing_error: Option<TimelineError>,
}

/// 재생 순서에서 다른 segment 다음에 오는 segment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextSegment {
    /// 호출 시점의 [`PlaylistTracker::segments`] 인덱스
    pub segment_index: usize,
    /// 해당 segment의 media sequence 번호
    pub media_sequence: u64,
    /// `false`면 sequence 공백이나 discontinuity가 직전 segment와 갈라놓음
    /// decoder가 timestamp 점프를 예상해야 함
    pub contiguous: bool,
}

/// 갱신을 거쳐 병합한 media playlist 하나의 이력
///
/// 조회가 돌려주는 인덱스는 그 호출 시점의 [`segments`](Self::segments) 기준
/// [`apply`](Self::apply)나 [`trim_before`](Self::trim_before)가 segment를 퇴출하면 밀림
/// 안정적인 식별자는 media sequence 번호
#[derive(Debug, Clone)]
pub struct PlaylistTracker {
    options: TrackerOptions,
    segments: Vec<Segment>,
    /// anchor가 첫 segment에만 있는 스트림에서 오래된 segment를 버리면 anchor도 함께 사라짐
    /// 그러면 남은 run 전체의 시간을 잃으므로, 버리기 전에 새로 맨 앞이 될 segment의
    /// 계산된 시작 시각을 여기 고정해 둠, (media sequence, unix nanos), 그 segment가 사라지면 함께 제거
    pinned_anchor: Option<(u64, i64)>,
    metadata: Option<MediaMetadata>,
    end_list: bool,
    /// `Err`면 아직 시간을 정할 수 없는 상태일 뿐 이력 자체는 유효, anchor가 오면 `Ok`가 됨
    index: Result<Vec<Interval<i64>>, TimelineError>,
    generation: u64,
}

impl Default for PlaylistTracker {
    fn default() -> Self {
        Self::new(TrackerOptions::default())
    }
}

impl PlaylistTracker {
    /// 빈 이력
    pub fn new(options: TrackerOptions) -> Self {
        Self {
            options,
            segments: Vec::new(),
            pinned_anchor: None,
            metadata: None,
            end_list: false,
            index: Err(TimelineError::EmptySnapshot),
            generation: 0,
        }
    }

    /// 갱신된 playlist 병합, 이미 추적 중인 segment는 같은 미디어여야 함
    ///
    /// 추적 중인 segment라도 나중 window에서 새로 배우는 것이 둘 있음
    /// 하나는 `EXT-X-DISCONTINUITY` 플래그, 창이 밀려 그 segment가 맨 앞에 오면 태그를 실을 자리가 없어서
    /// 앞서 본 window에서만 알 수 있음, 다른 하나는 선언 program time, window마다 자기 첫 segment에
    /// anchor를 두므로 창이 밀릴 때마다 다른 segment가 anchor를 얻음
    pub fn apply(&mut self, snapshot: &MediaSnapshot) -> Result<Update, TrackerError> {
        if let Some(tag) = snapshot.timeline_blockers().first() {
            return Err(TrackerError::UnsupportedTimingFeature { tag: tag.clone() });
        }
        let mut fresh = Vec::new();
        let mut learned = Vec::new();
        for segment in snapshot.segments() {
            match self.position(segment.media_sequence()) {
                Ok(existing) => {
                    let tracked = &self.segments[existing];
                    if !same_media(tracked, segment) {
                        return Err(TrackerError::ConflictingSegment {
                            media_sequence: segment.media_sequence(),
                        });
                    }
                    let flag = segment.discontinuity() && !tracked.discontinuity();
                    let anchor = tracked.declared_program_time().is_none()
                        && segment.declared_program_time().is_some();
                    if flag || anchor {
                        learned.push((existing, flag, segment.declared_program_time()));
                    }
                }
                Err(_) => fresh.push(segment.clone()),
            }
        }
        let added = fresh.len();
        let changed = added > 0 || !learned.is_empty();
        if changed {
            let fresh_sequences: Vec<u64> = fresh.iter().map(Segment::media_sequence).collect();
            let previous: Vec<(usize, bool, Option<UtcTimestamp>)> = learned
                .iter()
                .map(|&(i, _, _)| {
                    (
                        i,
                        self.segments[i].discontinuity,
                        self.segments[i].declared_program_time,
                    )
                })
                .collect();
            for &(existing, flag, anchor) in &learned {
                let tracked = &mut self.segments[existing];
                tracked.discontinuity |= flag;
                if tracked.declared_program_time.is_none() {
                    tracked.declared_program_time = anchor;
                }
            }
            self.segments.extend(fresh);
            self.segments.sort_by_key(Segment::media_sequence);
            self.rebuild();
            if let Some(media_sequence) = self.contradiction() {
                // 오류 계약대로 tracker가 그대로이도록 롤백
                for (i, flag, anchor) in previous {
                    self.segments[i].discontinuity = flag;
                    self.segments[i].declared_program_time = anchor;
                }
                self.segments
                    .retain(|segment| !fresh_sequences.contains(&segment.media_sequence()));
                self.rebuild();
                return Err(TrackerError::ConflictingSegment { media_sequence });
            }
        }
        let evicted = self.evict_to(self.options.max_segments);
        self.metadata = Some(snapshot.metadata().clone());
        self.end_list |= snapshot.metadata().end_list();
        if evicted > 0 {
            self.rebuild();
        }
        if changed || evicted > 0 {
            self.generation += 1;
        }
        Ok(Update {
            added,
            evicted,
            stale: added == 0,
            timing_error: self.index.as_ref().err().cloned(),
        })
    }

    /// `time` 이전에 끝나는 segment 제거, UTC 인덱스 필요
    pub fn trim_before(&mut self, time: UtcTimestamp) -> Result<usize, TimelineError> {
        let index = self.index.as_ref().map_err(Clone::clone)?;
        let keep_from = index.partition_point(|entry| entry.end <= time.as_unix_nanos());
        if keep_from == 0 {
            return Ok(0);
        }
        self.drop_oldest(keep_from);
        self.generation += 1;
        self.rebuild();
        Ok(keep_from)
    }

    /// 이력 전체 삭제
    pub fn reset(&mut self) {
        self.segments.clear();
        self.pinned_anchor = None;
        self.metadata = None;
        self.end_list = false;
        self.generation += 1;
        self.rebuild();
    }

    /// media sequence 순서의 추적 segment
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }
    /// 추적 중인 segment 수
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }
    /// 가장 최근 갱신의 playlist 수준 태그
    pub fn metadata(&self) -> Option<&MediaMetadata> {
        self.metadata.as_ref()
    }
    /// 어느 갱신에서든 `EXT-X-ENDLIST`를 받았는지 여부
    pub fn end_list(&self) -> bool {
        self.end_list
    }
    /// 이력이 바뀔 때마다 1씩 증가하는 값, 같은 generation이면 같은 인덱스
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// 가장 새로운 추적 segment
    pub fn live_edge(&self) -> Option<&Segment> {
        self.segments.last()
    }
    /// media sequence 번호로 찾은 segment와 현재 인덱스
    pub fn segment_by_sequence(&self, media_sequence: u64) -> Option<(usize, &Segment)> {
        let index = self.position(media_sequence).ok()?;
        Some((index, &self.segments[index]))
    }
    /// 주어진 sequence 다음에 재생할 segment, live edge에서는 `None`
    pub fn segment_after(&self, media_sequence: u64) -> Option<NextSegment> {
        let index = self
            .segments
            .partition_point(|segment| segment.media_sequence() <= media_sequence);
        let segment = self.segments.get(index)?;
        let contiguous = media_sequence.checked_add(1) == Some(segment.media_sequence())
            && !segment.discontinuity();
        Some(NextSegment {
            segment_index: index,
            media_sequence: segment.media_sequence(),
            contiguous,
        })
    }
    /// target이 가리키는 segment, 아직 있고 offset이 그 안에 있을 때만
    pub fn segment_for_target(&self, target: SeekTarget) -> Option<&Segment> {
        let (_, segment) = self.segment_by_sequence(target.media_sequence())?;
        (target.offset_in_segment().as_nanos() < segment.duration().as_nanos()).then_some(segment)
    }

    /// 이력이 덮는 UTC 구간, 첫 시작부터 마지막 종료(exclusive)까지
    pub fn window(&self) -> Result<(UtcTimestamp, UtcTimestamp), TimelineError> {
        let index = self.index.as_ref().map_err(Clone::clone)?;
        let (Some(first), Some(last)) = (index.first(), index.last()) else {
            return Err(TimelineError::EmptySnapshot);
        };
        Ok((
            UtcTimestamp::from_unix_nanos(first.start),
            UtcTimestamp::from_unix_nanos(last.end),
        ))
    }
    /// 인덱스로 찾은 추적 segment의 UTC 시작
    pub fn segment_start(&self, segment_index: usize) -> Result<UtcTimestamp, TimelineError> {
        let index = self.index.as_ref().map_err(Clone::clone)?;
        index
            .get(segment_index)
            .map(|entry| UtcTimestamp::from_unix_nanos(entry.start))
            .ok_or(TimelineError::EmptySnapshot)
    }
    /// 전체 이력에 대한 이진 탐색
    /// `Gap { segment_index: None }`은 discontinuity run 사이의 매핑되지 않은 구간과 본 적 없는 sequence 구간 둘 다
    pub fn locate(&self, time: UtcTimestamp) -> Result<LocateResult, TimelineError> {
        let index = self.index.as_ref().map_err(Clone::clone)?;
        Ok(locate(&self.segments, index, time.as_unix_nanos()))
    }
    /// 재생 순서에서 `time`에 가장 가까운 재생 가능 위치
    /// 재생 가능하면 그 위치, 아니면 다음 재생 가능 segment의 시작, 그것도 없으면 마지막 재생 가능 segment의 시작
    /// 재생 가능한 것이 없으면 `None`
    pub fn locate_nearest(&self, time: UtcTimestamp) -> Result<Option<SeekTarget>, TimelineError> {
        let index = self.index.as_ref().map_err(Clone::clone)?;
        let from = match locate(&self.segments, index, time.as_unix_nanos()) {
            LocateResult::Found(target) => return Ok(Some(target)),
            LocateResult::BeforeWindow => 0,
            LocateResult::AfterWindow => self.segments.len(),
            LocateResult::Gap {
                segment_index: Some(gap),
            } => gap + 1,
            LocateResult::Gap {
                segment_index: None,
            } => index.partition_point(|entry| entry.start <= time.as_unix_nanos()),
        };
        let forward = (from..self.segments.len()).find(|&i| !self.segments[i].gap());
        let backward = || {
            (0..from.min(self.segments.len()))
                .rev()
                .find(|&i| !self.segments[i].gap())
        };
        Ok(forward.or_else(backward).map(|i| {
            SeekTarget::new(
                i,
                self.segments[i].media_sequence(),
                ElapsedTime::from_nanos(0),
            )
        }))
    }

    /// 현재 인덱스가 스스로 모순되는 media sequence, 없으면 `None`
    /// anchor 누락은 모순이 아님, anchor가 올 때까지 시간이 없는 run일 뿐
    fn contradiction(&self) -> Option<u64> {
        match &self.index {
            Err(
                TimelineError::ConflictingTimeMapping { segment_index }
                | TimelineError::Overflow { segment_index },
            ) => Some(self.segments[*segment_index].media_sequence()),
            _ => None,
        }
    }

    fn position(&self, media_sequence: u64) -> Result<usize, usize> {
        self.segments
            .binary_search_by_key(&media_sequence, Segment::media_sequence)
    }

    fn evict_to(&mut self, limit: usize) -> usize {
        let excess = self.segments.len().saturating_sub(limit);
        if excess > 0 {
            self.drop_oldest(excess);
        }
        excess
    }

    /// 가장 오래된 `count`개 segment 제거
    /// 현재 인덱스가 유효하면 새로 가장 오래된 segment의 계산된 시작을 고정 anchor로 보관
    /// 그 run의 선언 anchor가 모두 사라져도 시간을 유지하기 위함
    fn drop_oldest(&mut self, count: usize) {
        if let (Ok(index), Some(survivor)) = (&self.index, self.segments.get(count)) {
            self.pinned_anchor = Some((survivor.media_sequence(), index[count].start));
        }
        self.segments.drain(..count);
        if let Some((sequence, _)) = self.pinned_anchor
            && self.position(sequence).is_err()
        {
            self.pinned_anchor = None;
        }
    }

    fn rebuild(&mut self) {
        let segments = &self.segments;
        self.index = if segments.is_empty() {
            Err(TimelineError::EmptySnapshot)
        } else {
            let pinned = self.pinned_anchor;
            relative_index(segments).and_then(|relative| {
                wall_clock_index(
                    segments,
                    &relative,
                    |i| starts_run(segments, i),
                    |i| {
                        segments[i]
                            .declared_program_time()
                            .map(UtcTimestamp::as_unix_nanos)
                            .or(pinned
                                .filter(|(sequence, _)| *sequence == segments[i].media_sequence())
                                .map(|(_, start)| start))
                    },
                    self.options.pdt_tolerance_nanos,
                )
            })
        };
    }
}

/// run 경계는 discontinuity, sequence 건너뜀(본 적 없는 segment), discontinuity sequence 변화
/// 마지막 것은 window가 태그를 실을 수 없었던 discontinuity를 드러냄
fn starts_run(segments: &[Segment], i: usize) -> bool {
    let Some(previous) = i.checked_sub(1).map(|p| &segments[p]) else {
        return true;
    };
    let current = &segments[i];
    current.discontinuity()
        || previous.media_sequence().checked_add(1) != Some(current.media_sequence())
        || previous.discontinuity_sequence() != current.discontinuity_sequence()
}

fn same_media(tracked: &Segment, incoming: &Segment) -> bool {
    tracked.uri() == incoming.uri()
        && tracked.duration() == incoming.duration()
        && tracked.discontinuity_sequence() == incoming.discontinuity_sequence()
        && tracked.gap() == incoming.gap()
        && tracked.byte_range() == incoming.byte_range()
        && match (
            tracked.declared_program_time(),
            incoming.declared_program_time(),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
}
