# CLAUDE.md

프로젝트 규칙은 [AGENTS.md](AGENTS.md)가 정본입니다. 이 파일은 놓치기 쉬운 것만 반복합니다.

## 버전

**개발 버전을 설치할 때는 version 을 `YYYY.M.#-{short commit hash}` 형태로 표시합니다.**

- `build.rs` 가 빌드 시점에 git 에서 만들어 넣습니다. `Cargo.toml` 의 `version` 에
  해시를 직접 적지 않습니다.
- 커밋하지 않은 변경이 있으면 `-dirty` 가 붙습니다 — `2026.8.2-2879224-dirty`.
- 릴리즈 태그를 그대로 체크아웃한 깨끗한 트리에서만 CalVer 버전만 나옵니다 — `2026.8.2`.
