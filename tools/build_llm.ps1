# 8.3 개념 관계 확인 엔진(src-tauri/llm, 보조 실행 파일)을 AVX2 로 빌드해 src-tauri/resources/llm/neoanki-llm.exe 에 넣는다 (커밋하지 않는다).
# 본체는 AVX2 없이 빌드하고(모든 CPU 호환), 이 엔진만 +avx2,+fma,+f16c 로 빌드한다. 본체가 실행할 때 CPU 를 확인하고 지원할 때만 부른다 (DECISIONS J11~).
# 릴리스 설치 파일을 만들기 전에 돌린다: 엔진이 없으면 본체의 릴리스 빌드(build.rs)가 멈춘다.
# 본체와 RUSTFLAGS 가 달라 target 폴더를 따로 쓴다 (같이 쓰면 서로 다시 빌드한다)
#   powershell -ExecutionPolicy Bypass -File tools\build_llm.ps1 [-TargetDir <폴더>]
param(
    [string]$TargetDir = (Join-Path $PSScriptRoot "..\src-tauri\llm\target")
)
$ErrorActionPreference = "Stop"

$Crate = Join-Path $PSScriptRoot "..\src-tauri\llm\Cargo.toml"
$Dest = Join-Path $PSScriptRoot "..\src-tauri\resources\llm"
$env:RUSTFLAGS = "-C target-feature=+avx2,+fma,+f16c"
cargo build --release --locked --manifest-path $Crate --target-dir $TargetDir
if ($LASTEXITCODE -ne 0) { throw "엔진 빌드 실패 (exit $LASTEXITCODE)" }
New-Item -ItemType Directory -Force $Dest | Out-Null
Copy-Item -Force (Join-Path $TargetDir "release\neoanki-llm.exe") (Join-Path $Dest "neoanki-llm.exe")
$f = Get-Item (Join-Path $Dest "neoanki-llm.exe")
"neoanki-llm.exe $($f.Length) bytes  SHA-256 $((Get-FileHash -Algorithm SHA256 $f.FullName).Hash.ToLower())"
