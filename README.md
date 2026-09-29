# hls-parser

HLS playlist(M3U8)를 파싱하고, 시각으로 segment를 찾는 Rust 라이브러리입니다.
> 해당 레포지토리는 타임 시프트 기능을 구현할 때 자료구조와 검색 알고리즘 차이에 따른 성능을 분석하기 위한 데모 작업입니다.

- playlist 텍스트를 받아 타입이 있는 모델로 바꿔서 사용할 수 있습니다. 
- Media playlist에는 시간 인덱스를 만들어 "재생 시작 2.5초 지점" 또는 "UTC 10:00:05"가 segment의 위치를 효율적이고 빠르게 찾을 수 있습니다.
- 라이브 playlist는 갱신마다 병합하여 한 번 본 segment를 재접근 가능합니다.


## 지원하는 기능

- RFC 8216 playlist를 파싱하고 Multivariant와 Media를 구분하고, 태그 값을 검증해 `Segment`, `EncryptionKey` 같은 타입으로 만듭니다.
- 파싱 뒤에도 입력한 문자열을 `SourceDocument`에서 그대로 보존합니다. 해석되지 않는 태그의 경우에도 모든 줄의 UTF-8 바이트 범위를 유지하고, 유실하지 않고 diagnostic으로 기록합니다.
- 상대 시간 검색을 지원하며 첫 segment부터의 경과 시간으로 segment를 찾습니다.
- UTC 시간 검색을 지원하며 `EXT-X-PROGRAM-DATE-TIME`을 anchor로 각 segment의 UTC 구간을 계산합니다.
- 라이브 갱신 병합. sliding window로 오는 갱신을 media sequence 기준으로 하나의 이력으로 합칩니다

## 지원하지 않는 기능

- 네트워크 요청, 미디어 다운로드, 복호화, 재생
- LL-HLS. `EXT-X-PART`, `EXT-X-SKIP` 같은 태그가 있으면 시간 인덱스를 생성하지 않습니다.
- playlist 재출력이나 편집
- 전체 RFC conformance 검증

## 시작하기

Rust 1.88 이상이 필요합니다.

```toml
[dependencies]
hls-core = { path = "../hls-parser/crates/hls-core" }
```

<details>
<summary><b>파싱과 상대 시간 검색</b></summary>

```rust
use hls_core::{ElapsedTime, LocateResult, ParseOptions, RelativeTimeline, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:5.5,\na.ts\n#EXTINF:4,\nb.ts\n";
    let document = parse(text, &ParseOptions::default())?;

    // parse 결과가 Multivariant일 수 있으므로 media()는 Option
    let media = document.media().ok_or("media playlist가 아님")?.clone();
    let timeline = RelativeTimeline::build(media)?;

    match timeline.locate(ElapsedTime::from_nanos(7_000_000_000)) {
        LocateResult::Found(target) => {
            // segment 1, offset 1_500_000_000ns
            println!("{} {}", target.segment_index(), target.offset_in_segment().as_nanos());
        }
        LocateResult::Gap { .. } => println!("EXT-X-GAP 구간"),
        LocateResult::BeforeWindow | LocateResult::AfterWindow => println!("범위 밖"),
    }
    Ok(())
}
```

시간 값은 모두 정수 나노초입니다. `EXTINF:5.5`는 `5_500_000_000`으로 정확히 변환되고, 소수 10자리 이상은 오류로 처리합니다.

</details>

<details>
<summary><b>UTC 시간 검색</b></summary>

```rust
use hls_core::{ParseOptions, UtcTimestamp, WallClockTimeline, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n\
                #EXT-X-PROGRAM-DATE-TIME:2026-09-27T10:00:00Z\n\
                #EXTINF:4,\na.ts\n#EXTINF:4,\nb.ts\n";
    let document = parse(text, &ParseOptions::default())?;
    let timeline = WallClockTimeline::build(document.media().ok_or("media playlist가 아님")?.clone())?;

    let at = UtcTimestamp::parse_rfc3339("2026-09-27T10:00:05Z")?;
    println!("{:?}", timeline.locate(at)); // Found: segment 1, offset 1초
    Ok(())
}
```

`WallClockTimeline::build`는 anchor가 부족하면 `TimelineError::MissingTimeMapping`으로 실패합니다. 이때도 `RelativeTimeline`은 만들 수 있습니다.

</details>

<details>
<summary><b>라이브 playlist 병합</b></summary>

```rust
use hls_core::{ParseOptions, PlaylistTracker, UtcTimestamp, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut tracker = PlaylistTracker::default();

    // 매 갱신마다 응답을 파싱해 apply에 넘김
    for text in fetch_refreshes() {
        let document = parse(&text, &ParseOptions::default())?;
        let update = tracker.apply(document.media().ok_or("media playlist가 아님")?)?;
        println!("added {} evicted {}", update.added, update.evicted);
    }

    // 창 밖으로 밀려난 시각도 이력에 남아 있으면 찾음
    let at = UtcTimestamp::parse_rfc3339("2026-09-27T10:00:01Z")?;
    println!("{:?}", tracker.locate(at)?);
    Ok(())
}

fn fetch_refreshes() -> Vec<String> {
    // 네트워크 요청은 이 라이브러리 밖의 일
    vec![
        "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n\
         #EXT-X-PROGRAM-DATE-TIME:2026-09-27T10:00:00Z\n\
         #EXTINF:2,\nseg0.ts\n#EXTINF:2,\nseg1.ts\n#EXTINF:2,\nseg2.ts\n".into(),
        "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:2\n\
         #EXT-X-PROGRAM-DATE-TIME:2026-09-27T10:00:04Z\n\
         #EXTINF:2,\nseg2.ts\n#EXTINF:2,\nseg3.ts\n#EXTINF:2,\nseg4.ts\n".into(),
    ]
}
```

`apply`는 트랜잭션입니다. 이미 추적 중인 segment와 URI, 길이, byte range가 다르거나 anchor가 기존 시간축과 불일치 한다면 `TrackerError::ConflictingSegment`로 거절하고 원본 데이터를 유지하며 로그를 기록합니다.

</details>

<details>
<summary><b>상대 URI 해석</b></summary>

```rust
let absolute = hls_core::resolve_uri("https://cdn.example/live/index.m3u8", "../seg0.ts")?;
// https://cdn.example/seg0.ts
```

`base`에는 리다이렉트가 끝난 최종 응답 URL을 넘깁니다.

</details>


## 설계 구성

| 경로 | 역할 |
|---|---|
| `crates/hls-core` | 파싱, 도메인 모델, 시간 인덱스, 라이브 tracker. Rust에서는 이 crate만 사용합니다. |
| `crates/hls-ffi` | UniFFI를 통해 `hls-core`를 다른 언어에서도 쓸 수 있게 하는 코드. Rust에서만 쓴다면 필요 없음 |
| `fixtures` | 테스트용 playlist |

