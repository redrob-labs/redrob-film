# 레드롭 필름

레드롭 필름은 에이전트가 편집하는 영상 편집기입니다. 원하는 컷을 말로 설명하면 에이전트가 만듭니다. 창 하나에
모드 둘: **Edit**(타임라인 NLE)와 **Effects**(모션 그래픽·합성), 그리고 어느 모드에서나 옆에 열리는 AI 도크.

English: [README.md](./README.md)

> **출처.** 레드롭 필름은 오픈소스 코드에서 출발해 이 저장소에서 독자적으로 개발합니다. 그 코드와는 더 이상
> 동기화하지 않습니다. 어디서 왔는지, 기존 파일을 읽으려고 남겨 둔 옛 이름이 무엇인지는
> [SOURCE_ORIGINS.md](./SOURCE_ORIGINS.md)에 있습니다. 원 저작권 표기는 [NOTICE](./NOTICE)에 그대로 두고 그 아래에
> 우리 줄을 추가했습니다. `assets/brand/`의 레드롭 마크는 레드롭의 상표입니다.

## 무엇인가

- Edit 모드: 순수 Rust로 만든 비선형 영상 편집기. H.264·HEVC·ProRes·AAC 코덱을 자체 구현하고, GPU 합성기가 선형 광에서 작업합니다.
- Effects 모드: 같은 창, 같은 프로젝트 번들 안에서 모션 그래픽과 합성(레이어·키프레임·이펙트·표현식·텍스트).
- Mode 메뉴 핸드오프: **Send Selection to Effects**는 선택한 클립(또는 인/아웃 구간, 또는 시퀀스 전체)을 새 컴포지션으로
  만들고, **Place Composition on Timeline**은 활성 컴포지션을 Effects 렌더러가 실시간으로 그리는 클립으로 타임라인에
  놓습니다. Edit/Effects 전환은 상단 바 버튼이나 단축키로 합니다.
- 앱의 모든 동작이 명령(command)입니다. UI·CLI·JSON 제어 채널·MCP 서버가 같은 명령을 호출하므로 에이전트가 사람과 똑같이 자르고, 다듬고, 색을 잡고, 믹스하고, 내보낼 수 있습니다.
- **Redrob Code가 내장돼 있습니다.** AI 도크(상단 바의 AI 버튼, Mode ▸ Show AI, Ctrl+Shift+A)에서 Redrob Code와 대화하면 두 모드 모두에서 같은 명령으로 열린 프로젝트를 편집합니다. 모델 호출은 연결한 레드롭 워크스페이스에 과금되고(Help ▸ Connect to Redrob…), 편집 자체는 무료이며 계정을 묻지 않습니다. 다른 에이전트는 `redrob-film-cli mcp`로 연결합니다([docs/agents.md](./docs/agents.md)).
- macOS·Windows·Linux 네이티브.

## 다운로드

가장 쉬운 방법은 [Redrob Launcher](https://github.com/redrob-labs/redrob-launcher)입니다. 레드롭 필름을 설치하고 최신으로
유지하며 모든 레드롭 앱을 한 워크스페이스로 로그인합니다. 직접 받으려면 [릴리스 페이지](https://github.com/redrob-labs/redrob-film/releases)에서
받습니다. 직접 설치한 사본은 스스로 업데이트를 확인합니다(상단 바의 Updates 메뉴, `REDROB_FILM_NO_UPDATE_CHECK=1`로 자동 확인 해제).

| 플랫폼 | 다운로드 |
|---|---|
| Linux x86_64 | `redrob-film-<version>-linux-x86_64.tar.gz`(`./install.sh` 실행, sudo 불필요) 또는 `redrob-film-<version>-linux-x86_64.deb` |
| macOS(Apple silicon·Intel) | `redrob-film-<version>-macos-universal.dmg` |
| Windows x64 | `redrob-film-<version>-windows-x64.msi` 또는 `redrob-film-<version>-windows-x64-portable.zip` |

릴리스의 `SHA256SUMS.txt`에 모든 파일의 체크섬이 있습니다. 자세한 것은 [docs/releasing.md](./docs/releasing.md).

## 시작하기

```sh
cargo run --release -p redrob-film-app                     # 데스크톱 앱(redrob-film, 데모 프로젝트)
cargo run --release -p redrob-film-app -- --control 9876   # JSON-lines 제어 서버 포함
cargo run --release -p redrob-film-cli -- commands     # 엔진 명령 전체 목록(redrob-film-cli)
cargo run --release -p redrob-film-cli -- mcp          # MCP 서버(헤드리스)
```

Cargo 패키지는 `redrob-film-*`(Edit 모드, 앱, CLI)와 `redrob-effects-*`(Effects 편집기, `editors/effects/`)입니다. 제품 실행 파일은 `redrob-film` 하나이고, 명령줄 도구 `redrob-film-cli`가 따로 있습니다. 앱 ID는 `ai.redrob.film`입니다.

## 레드롭 코드에서 제어

데스크톱 앱이 열려 있는 동안 공유 레드롭 코드 데몬에 스스로 등록되므로, 어느 레드롭 표면의 에이전트든(휴대폰 채팅 포함) `app:film` 도구로 열린 프로젝트를 다룰 수 있습니다.

- `project_info` - 프로젝트 파일 경로, 활성 시퀀스의 길이·프레임 레이트·해상도·트랙과 클립 수.
- `open_project` - 프로젝트 파일(`.rfilm`)을 열고 설명합니다.
- `list_clips` - 시퀀스의 클립 목록(트랙 하나로 좁힐 수 있음): id, 트랙, 이름, 타임라인 인/아웃과 소스 인/아웃(초).
- `trim_clip` - 클립의 인/아웃 점을 새 타임라인 위치(초)로 옮깁니다.
- `split_clip` - 클립을 타임라인 위치에서 둘로 자르고 조각들의 id를 답합니다.
- `set_playhead` - 재생 헤드를 타임라인 위치(초)로 옮깁니다.
- `render` - 앱 자체의 내보내기 작업을 시작합니다(출력 경로와 프리셋은 선택). 작업 id와 출력 경로를 답합니다.
- `render_status` - 렌더 작업의 진행률과, 끝난 뒤에는 기록된 파일 경로.

도구는 메뉴와 똑같이 실제 편집기에 대해 실행됩니다. 데몬이 없으면 앱은 평소처럼 동작합니다.

## 브랜치와 기여

- 기본 브랜치 `develop`, 릴리스 브랜치 `main`. 브랜치 이름은 `feat/ fix/ chore/ docs/ test/ refactor/ perf/ ci/ build/ sync/ hotfix/ release/` 접두사를 따르며, redrob-labs 공용 gitflow 워크플로가 검사합니다(모든 레포가 같은 목록).
- 조직 공통 규약: [redrob-labs/.github](https://github.com/redrob-labs/.github)의 CONTRIBUTING.ko.md와 FORKS.ko.md.

## 라이선스

MIT 또는 Apache-2.0 중 선택([LICENSE-MIT](./LICENSE-MIT), [LICENSE-APACHE](./LICENSE-APACHE)).
저작권 표기는 [NOTICE](./NOTICE)에 있습니다. 원 저작자 표기 아래에 우리 표기를 추가했습니다:

```
Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
Copyright (c) 2026-present Janghoon Lee (Redrob) and contributors
```

레드롭 이름과 마크는 이 라이선스에 포함되지 않습니다([assets/brand/README.md](./assets/brand/README.md)).
자산별 귀속은 [ATTRIBUTION.md](./ATTRIBUTION.md)에 있습니다.
