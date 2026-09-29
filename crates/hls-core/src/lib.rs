//! HLS playlist 파싱과 시각 기준 segment 검색
//! 네트워킹과 플레이어는 없음, playlist 텍스트를 받아 모델과 시간 인덱스를 돌려줄 뿐
//!
//! ```
//! use hls_core::{parse, ParseOptions, RelativeTimeline, ElapsedTime, LocateResult};
//! let doc = parse("#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:5.5,\na.ts\n", &ParseOptions::default())?;
//! let timeline = RelativeTimeline::build(doc.media().unwrap().clone())?;
//! assert!(matches!(timeline.locate(ElapsedTime::from_nanos(1)), LocateResult::Found(_)));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! 공개 타입은 아래에 하나씩 적음, glob re-export를 쓰지 않는 이유는 타입 하나를 추가할 때
//! 그것이 공개 API가 된다는 사실을 한 번 더 보게 하려는 것

mod attributes;
mod diagnostic;
mod domain;
mod parser;
mod source;
mod time;
mod timeline;
mod tracker;

pub use diagnostic::{Diagnostic, ErrorCode, ParseError};
pub use domain::{
    Attribute, ByteRange, EncryptionKey, EncryptionMethod, InitializationMap, MediaMetadata,
    MediaSnapshot, MultivariantPlaylist, ParsedDocument, Playlist, PlaylistKind, PlaylistType,
    Rendition, Segment, Variant,
};
pub use parser::{ParseOptions, parse};
pub use source::{LineKind, RawLine, SourceDocument, SourceSpan};
pub use time::{ElapsedTime, NANOS_PER_SECOND, SegmentDuration, UtcTimestamp};
pub use timeline::{LocateResult, RelativeTimeline, SeekTarget, TimelineError, WallClockTimeline};
pub use tracker::{NextSegment, PlaylistTracker, TrackerError, TrackerOptions, Update};

/// 최종 playlist 응답 URL(리다이렉트 이후) 기준의 URI 해석
/// 네트워크 요청 없음, 도메인 모델의 원래 URI는 그대로 유지
pub fn resolve_uri(base: &str, uri: &str) -> Result<String, url::ParseError> {
    Ok(url::Url::parse(base)?.join(uri)?.to_string())
}
