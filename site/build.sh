#!/bin/sh
# 공개 사이트(홈 · 개인정보 처리방침 · 이용약관)를 만든다. 저장소 루트에서 돌린다. Google OAuth 동의 화면이 이 주소들을 쓴다.
# SPA 라 같은 index.html 을 /privacy/ · /terms/ 에도 둔다. 검토 봇이 그 주소를 바로 열어도 200 으로 본문을 받는다.
# 배포 (Cloudflare Pages, 프로젝트 addiem):
#   sh site/build.sh _site && npx wrangler pages deploy _site --project-name addiem --branch main
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
