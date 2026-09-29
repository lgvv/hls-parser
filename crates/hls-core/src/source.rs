//! 입력 텍스트 원본과 각 줄의 위치
//! 오류와 diagnostic이 "몇 번째 줄의 어디"를 가리킬 수 있게 하기 위한 것

use std::sync::Arc;

/// 원문에서의 위치, 바이트 범위 `[start, end)`와 줄 번호
/// 예: 세 번째 줄 `#EXTINF:5.5,`는 line 3, start는 앞 두 줄과 개행의 바이트 수
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    /// 시작 바이트 오프셋, 이 바이트 포함
    pub start: usize,
    /// 종료 바이트 오프셋, 이 바이트 제외
    pub end: usize,
    /// 사람이 읽는 줄 번호, 1부터
    pub line: usize,
}

/// 원문 줄의 문법 분류, 의미 해석 전에 결정
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// `#EXTM3U` 줄
    Header,
    /// `#EXT`로 시작하는 줄
    Tag,
    /// `#`로 시작하지만 태그가 아닌 줄
    Comment,
    /// segment 또는 variant URI
    Uri,
    /// 빈 줄
    Blank,
}

/// 종결자를 뺀 원문 줄 하나
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLine {
    /// 문법 분류
    pub kind: LineKind,
    /// `\r\n` 또는 `\n`을 뺀 위치
    pub span: SourceSpan,
}

/// 입력 텍스트 원본과 줄 목록, 파싱 결과와 함께 보관
/// 텍스트는 읽기 전용 복사본이라 모델을 고쳐서 다시 출력하는 용도는 아님
#[derive(Debug, Clone)]
pub struct SourceDocument {
    pub(crate) text: Arc<str>,
    pub(crate) lines: Vec<RawLine>,
}

impl SourceDocument {
    /// 파싱한 그대로의 입력
    pub fn text(&self) -> &str {
        &self.text
    }
    /// 입력 순서의 줄 목록
    pub fn lines(&self) -> &[RawLine] {
        &self.lines
    }
    /// span 안의 텍스트, 잘못된 범위나 UTF-8 code point 중간의 오프셋이면 `None`
    pub fn text_at(&self, span: SourceSpan) -> Option<&str> {
        self.text.get(span.start..span.end)
    }
}
