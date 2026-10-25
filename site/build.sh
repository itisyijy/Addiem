#!/bin/sh
# 공개 사이트(홈 · 개인정보 처리방침 · 이용약관)를 만든다. 저장소 루트에서 돌린다. Google OAuth 동의 화면이 이 주소들을 쓴다.
# SPA 라 같은 index.html 을 /privacy/ · /terms/ 에도 둔다. 검토 봇이 그 주소를 바로 열어도 200 으로 본문을 받는다.
# 배포 (Cloudflare Pages 프로젝트 addiem → https://addiem.pages.dev):
#   sh site/build.sh _site && npx wrangler@4.149.0 pages deploy _site --project-name addiem --branch main
# `wrangler pages project create` 는 쓰지 않는다. 이 버전은 Workers 프로젝트를 만들면서 저장소 루트를 앱으로 알아보고
# package.json · vite.config.ts 를 고친 뒤 앱 화면을 배포한다 (2026-10-25). 프로젝트는 이미 있다
set -eu
out=${1:-_site}
rm -rf "$out"
mkdir -p "$out/img" "$out/fonts" "$out/privacy" "$out/terms"
cp site/index.html "$out/index.html"
cp site/index.html "$out/404.html"
sed 's|<title>[^<]*</title>|<title>개인정보 처리방침 · Addiem</title>|' site/index.html > "$out/privacy/index.html"
sed 's|<title>[^<]*</title>|<title>이용약관 · Addiem</title>|' site/index.html > "$out/terms/index.html"
cp design/logo/addiem-lockup.svg design/logo/addiem-lockup-dark.svg design/logo/addiem-mark.svg design/logo/addiem-mark-dark.svg design/logo/addiem-icon.svg "$out/img/"
cp docs/images/readme/hero.png docs/images/readme/deck-goal.png docs/images/readme/source-outline.png docs/images/readme/card-check.png "$out/img/"
# ponytail: 글꼴 전체(굵기당 약 770KB)를 쓴다. 느리면 pyftsubset 으로 페이지에 쓰는 글자만 남긴다
cp src/assets/fonts/pretendard/Pretendard-Regular.woff2 src/assets/fonts/pretendard/Pretendard-SemiBold.woff2 src/assets/fonts/pretendard/LICENSE.txt "$out/fonts/"
