//! 파싱 결과 모델
//!
//! 타입이 두 부류로 나뉨
//! 필드를 어떻게 조합해도 말이 되는 값(`Attribute`, `ByteRange`, `EncryptionKey`, `Diagnostic`,
//! `SourceSpan`)은 필드 공개
//! 파싱 중 서로 맞춰 검증한 필드를 가진 값(`Segment`, `MediaSnapshot`, `Variant`)은 getter만 공개,
//! 밖에서 media sequence만 바꾼 segment 같은 모순된 값을 만들 수 없게 하기 위함

use crate::{Diagnostic, SegmentDuration, SourceDocument, SourceSpan, UtcTimestamp};
use std::sync::Arc;

/// RFC 8216의 두 playlist 종류 중 하나
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistKind {
    /// variant stream과 rendition 목록, segment 없음
    Multivariant,
    /// media segment 목록
    Media,
}

/// 파싱된 문서의 타입 있는 payload
#[derive(Debug, Clone)]
pub enum Playlist {
    /// Multivariant(master) playlist
    Multivariant(MultivariantPlaylist),
    /// Media playlist, timeline이 문서보다 오래 살도록 공유
    Media(Arc<MediaSnapshot>),
}

/// 파싱에 성공한 playlist와 그 원문
#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub(crate) source: SourceDocument,
    pub(crate) playlist: Playlist,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) omitted_diagnostics: usize,
}

impl ParsedDocument {
    /// 원문 텍스트와 줄 인덱스
    pub fn source(&self) -> &SourceDocument {
        &self.source
    }
    /// 타입 있는 모델
    pub fn playlist(&self) -> &Playlist {
        &self.playlist
    }
    /// 파싱한 playlist 종류
    pub fn kind(&self) -> PlaylistKind {
        match self.playlist {
            Playlist::Multivariant(_) => PlaylistKind::Multivariant,
            Playlist::Media(_) => PlaylistKind::Media,
        }
    }
    /// media snapshot, Multivariant playlist면 `None`
    pub fn media(&self) -> Option<&Arc<MediaSnapshot>> {
        if let Playlist::Media(media) = &self.playlist {
            Some(media)
        } else {
            None
        }
    }
    /// 의미 해석 없이 보존한 태그, `ParseOptions::max_diagnostics`까지만
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    /// 제한에 걸려 버린 diagnostic 수
    pub fn omitted_diagnostics(&self) -> usize {
        self.omitted_diagnostics
    }
}

/// attribute list의 `NAME=value` 쌍 하나, 원문 그대로 보존
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// 대문자 attribute 이름
    pub name: String,
    /// 따옴표를 뺀 값
    pub value: String,
    /// 원문에서 값이 따옴표로 감싸였는지 여부
    pub quoted: bool,
}

/// `EXT-X-STREAM-INF` 항목과 그 URI
#[derive(Debug, Clone)]
pub struct Variant {
    pub(crate) uri: String,
    pub(crate) bandwidth: u64,
    pub(crate) attributes: Vec<Attribute>,
}
impl Variant {
    /// 원문에 쓰인 URI 줄, 해석은 [`crate::resolve_uri`]
    pub fn uri(&self) -> &str {
        &self.uri
    }
    /// 검증된 `BANDWIDTH` attribute
    pub fn bandwidth(&self) -> u64 {
        self.bandwidth
    }
    /// `BANDWIDTH`를 포함한 모든 attribute
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
}

/// `EXT-X-MEDIA` 항목
#[derive(Debug, Clone)]
pub struct Rendition {
    pub(crate) attributes: Vec<Attribute>,
}
impl Rendition {
    /// 모든 attribute, `TYPE`, `GROUP-ID`, `NAME`은 항상 존재
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
}

/// Multivariant playlist의 variant stream과 rendition, 원문 순서
#[derive(Debug, Clone)]
pub struct MultivariantPlaylist {
    pub(crate) variants: Vec<Variant>,
    pub(crate) renditions: Vec<Rendition>,
}
impl MultivariantPlaylist {
    /// 원문 순서의 `EXT-X-STREAM-INF` 항목
    pub fn variants(&self) -> &[Variant] {
        &self.variants
    }
    /// 원문 순서의 `EXT-X-MEDIA` 항목
    pub fn renditions(&self) -> &[Rendition] {
        &self.renditions
    }
}

/// `EXT-X-PLAYLIST-TYPE` 값
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistType {
    /// segment가 뒤에만 추가되는 playlist
    Event,
    /// 변하지 않는 playlist
    Vod,
}

/// Media playlist의 playlist 수준 태그
#[derive(Debug, Clone)]
pub struct MediaMetadata {
    pub(crate) target_duration: u64,
    pub(crate) media_sequence: u64,
    pub(crate) discontinuity_sequence: u64,
    pub(crate) end_list: bool,
    pub(crate) playlist_type: Option<PlaylistType>,
    pub(crate) version: Option<u64>,
    pub(crate) i_frames_only: bool,
}
impl MediaMetadata {
    /// `EXT-X-TARGETDURATION`(정수 초)
    pub fn target_duration_seconds(&self) -> u64 {
        self.target_duration
    }
    /// `EXT-X-MEDIA-SEQUENCE`, 없으면 0
    pub fn media_sequence(&self) -> u64 {
        self.media_sequence
    }
    /// `EXT-X-DISCONTINUITY-SEQUENCE`, 없으면 0
    pub fn discontinuity_sequence(&self) -> u64 {
        self.discontinuity_sequence
    }
    /// `EXT-X-ENDLIST` 존재 여부
    pub fn end_list(&self) -> bool {
        self.end_list
    }
    /// `EXT-X-PLAYLIST-TYPE`, 있을 때만
    pub fn playlist_type(&self) -> Option<PlaylistType> {
        self.playlist_type
    }
    /// `EXT-X-VERSION`, 있을 때만
    pub fn version(&self) -> Option<u64> {
        self.version
    }
    /// `EXT-X-I-FRAMES-ONLY` 존재 여부
    /// 각 segment는 I-frame 하나, 길이는 다음 I-frame까지, seekbar 썸네일 용도
    pub fn i_frames_only(&self) -> bool {
        self.i_frames_only
    }
}

/// 해석이 끝난 byte range, `offset + length`의 오버플로 없음을 검증
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// 첫 바이트
    pub offset: u64,
    /// 바이트 수, 항상 양수
    pub length: u64,
}

/// `EXT-X-KEY`의 `METHOD` attribute
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptionMethod {
    /// `AES-128`
    Aes128,
    /// `SAMPLE-AES`
    SampleAes,
    /// 그 외 method, 원문 그대로 보존
    Other(String),
}

/// method가 `NONE`이 아닌 `EXT-X-KEY` 하나
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionKey {
    /// 암호화 method
    pub method: EncryptionMethod,
    /// 원문에 쓰인 key URI
    pub uri: String,
    /// 원문에 쓰인 `IV` attribute, 최대 128비트 16진수로 검증
    pub iv: Option<String>,
    /// `KEYFORMAT`, 기본값 `identity`
    pub key_format: String,
    /// 원문에 쓰인 `KEYFORMATVERSIONS`
    pub key_format_versions: Option<String>,
}

/// `EXT-X-MAP` initialization section
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializationMap {
    /// 원문에 쓰인 map URI
    pub uri: String,
    /// `BYTERANGE` attribute, 있을 때만
    pub byte_range: Option<ByteRange>,
    pub(crate) keys: Arc<[Arc<EncryptionKey>]>,
}
impl InitializationMap {
    /// `MAP` 선언 시점의 암호화 상태, 이후 media segment와 다를 수 있음
    pub fn effective_keys(&self) -> impl ExactSizeIterator<Item = &EncryptionKey> {
        self.keys.iter().map(Arc::as_ref)
    }
}

/// 적용된 태그 상태를 포함한 media segment 하나
#[derive(Debug, Clone)]
pub struct Segment {
    pub(crate) uri: String,
    pub(crate) duration: SegmentDuration,
    pub(crate) title: String,
    pub(crate) media_sequence: u64,
    pub(crate) discontinuity_sequence: u64,
    pub(crate) discontinuity: bool,
    pub(crate) declared_program_time: Option<UtcTimestamp>,
    pub(crate) bitrate_kbps: Option<u64>,
    pub(crate) gap: bool,
    pub(crate) byte_range: Option<ByteRange>,
    pub(crate) keys: Arc<[Arc<EncryptionKey>]>,
    pub(crate) map: Option<Arc<InitializationMap>>,
    pub(crate) span: SourceSpan,
}
impl Segment {
    /// 원문에 쓰인 URI 줄, 해석은 [`crate::resolve_uri`]
    pub fn uri(&self) -> &str {
        &self.uri
    }
    /// `EXTINF` 길이
    pub fn duration(&self) -> SegmentDuration {
        self.duration
    }
    /// `EXTINF` 제목, 빈 문자열 가능
    pub fn title(&self) -> &str {
        &self.title
    }
    /// 이 segment의 media sequence 번호
    pub fn media_sequence(&self) -> u64 {
        self.media_sequence
    }
    /// 이 segment의 discontinuity sequence 번호
    pub fn discontinuity_sequence(&self) -> u64 {
        self.discontinuity_sequence
    }
    /// 이 segment 앞의 `EXT-X-DISCONTINUITY` 존재 여부
    pub fn discontinuity(&self) -> bool {
        self.discontinuity
    }
    /// 명시적 `EXT-X-PROGRAM-DATE-TIME`, 추론 값 제외
    pub fn declared_program_time(&self) -> Option<UtcTimestamp> {
        self.declared_program_time
    }
    /// 적용 중인 `EXT-X-BITRATE`(kbps), packager가 선언했을 때만
    pub fn bitrate_kbps(&self) -> Option<u64> {
        self.bitrate_kbps
    }
    /// 이 segment 앞의 `EXT-X-GAP` 존재 여부
    pub fn gap(&self) -> bool {
        self.gap
    }
    /// 해석이 끝난 `EXT-X-BYTERANGE`, 있을 때만
    pub fn byte_range(&self) -> Option<ByteRange> {
        self.byte_range
    }
    /// 이 segment에 적용 중인 key, `KEYFORMAT`당 하나, 비어 있으면 암호화 없음
    pub fn effective_keys(&self) -> impl ExactSizeIterator<Item = &EncryptionKey> {
        self.keys.iter().map(Arc::as_ref)
    }
    /// 이 segment에 적용 중인 `EXT-X-MAP`
    pub fn effective_map(&self) -> Option<&InitializationMap> {
        self.map.as_deref()
    }
    /// `EXTINF` 줄부터 URI 줄까지
    pub fn source_span(&self) -> SourceSpan {
        self.span
    }
}

/// 불변 media playlist, timeline이 복사 없이 색인
#[derive(Debug, Clone)]
pub struct MediaSnapshot {
    pub(crate) metadata: MediaMetadata,
    pub(crate) segments: Vec<Segment>,
    pub(crate) timeline_blockers: Vec<String>,
}
impl MediaSnapshot {
    /// playlist 수준 태그
    pub fn metadata(&self) -> &MediaMetadata {
        &self.metadata
    }
    /// 원문 순서의 segment
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }
    /// segment 수
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }
    /// `index`번째 segment, 첫 segment가 0
    pub fn segment_at(&self, index: usize) -> Option<&Segment> {
        self.segments.get(index)
    }
    /// 시간 인덱스를 만들면 틀린 답이 나오게 하는 태그, 예: `EXT-X-SKIP`
    /// SKIP은 segment를 생략했다는 뜻이라 EXTINF를 더하기만 하면 시간이 어긋남
    pub fn timeline_blockers(&self) -> &[String] {
        &self.timeline_blockers
    }
}
