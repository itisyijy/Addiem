# AU20 읽어주기 음성(Supertonic 3 fp32 · F1)과 ONNX Runtime 을 src-tauri/resources/tts 에 채운다 (약 400MB, 커밋하지 않는다).
# 기본: 이미 받아 둔 폴더(%LOCALAPPDATA%\addiem-dev\tts, 없으면 예전 이름 폴더 neoanki-dev\tts)에서 복사한다. -Download 를 주면 고정 리비전에서 받는다.
# 파일마다 SHA-256 을 확인하고, 맞지 않으면 멈춘다. 이미 맞는 파일은 건너뛴다.
#   powershell -ExecutionPolicy Bypass -File tools\fetch_tts.ps1 [-Source <폴더>] [-Download]
param(
    [string]$Source = "",
    [switch]$Download
)
$ErrorActionPreference = "Stop"
if (-not $Source) {
    $Source = Join-Path $env:LOCALAPPDATA "addiem-dev\tts"
    if (-not (Test-Path $Source)) { $Source = Join-Path $env:LOCALAPPDATA "neoanki-dev\tts" }
}

$Dest = Join-Path $PSScriptRoot "..\src-tauri\resources\tts"
# Hugging Face Supertone/supertonic-3 (OpenRAIL-M) 고정 리비전 (2026-05-18)
$HfBase = "https://huggingface.co/Supertone/supertonic-3/resolve/3cadd1ee6394adea1bd021217a0e650ede09a323"
$OrtZip = "https://github.com/microsoft/onnxruntime/releases/download/v1.31.0/onnxruntime-win-x64-1.31.0.zip"
$OrtZipSha = "3958d8a44984160692c1b4ffd0de3cb48f8c39f9d34bb31e433db06e30b82195"

# 넣을 곳(tts/ 안), 받아 둔 폴더 안의 이름, 받는 주소(모델) 또는 zip 안의 이름(ONNX Runtime), SHA-256
$Files = @(
    @("supertonic-3/onnx/duration_predictor.onnx", "supertonic-3-fp32/onnx/duration_predictor.onnx", "hf:onnx/duration_predictor.onnx", "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db"),
    @("supertonic-3/onnx/text_encoder.onnx", "supertonic-3-fp32/onnx/text_encoder.onnx", "hf:onnx/text_encoder.onnx", "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff"),
    @("supertonic-3/onnx/vector_estimator.onnx", "supertonic-3-fp32/onnx/vector_estimator.onnx", "hf:onnx/vector_estimator.onnx", "883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c"),
    @("supertonic-3/onnx/vocoder.onnx", "supertonic-3-fp32/onnx/vocoder.onnx", "hf:onnx/vocoder.onnx", "085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba"),
    @("supertonic-3/onnx/tts.json", "supertonic-3-fp32/onnx/tts.json", "hf:onnx/tts.json", "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09"),
    @("supertonic-3/onnx/unicode_indexer.json", "supertonic-3-fp32/onnx/unicode_indexer.json", "hf:onnx/unicode_indexer.json", "9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f"),
    @("supertonic-3/voice_styles/F1.json", "supertonic-3-fp32/voice_styles/F1.json", "hf:voice_styles/F1.json", "bbdec6ee00231c2c742ad05483df5334cab3b52fda3ba38e6a07059c4563dbc2"),
    @("supertonic-3/LICENSE", "supertonic-3-fp32/LICENSE", "hf:LICENSE", "0d944a9110fed9a9602d60e0423a272903e7bd21ab060490774efc77c2275e9f"),
    @("onnxruntime/onnxruntime.dll", "onnxruntime-1.31.0/onnxruntime.dll", "zip:onnxruntime-win-x64-1.31.0/lib/onnxruntime.dll", "0c42dc4e278f0968fdf9ebf1b1e54a74c657a9942e70803e453c91993451cf4c"),
    @("onnxruntime/LICENSE", "onnxruntime-1.31.0/LICENSE", "zip:onnxruntime-win-x64-1.31.0/LICENSE", "c250d6278f0b47a6439fb7592b08b58a55eb9f535aa49a1db63211c3f982b674"),
    @("onnxruntime/ThirdPartyNotices.txt", "onnxruntime-1.31.0/ThirdPartyNotices.txt", "zip:onnxruntime-win-x64-1.31.0/ThirdPartyNotices.txt", "c53a76501ef60db6f865f20599f220761201ac4057683acefdab37d861b86622")
)

function Sha($path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLower() }

function Check($path, $sha) {
    $got = Sha $path
    if ($got -ne $sha) { throw "SHA-256 이 다르다: $path`n  기대 $sha`n  실제 $got" }
}

$work = $null
function OrtFile($inner) {
    # zip 은 한 번만 받는다
    if (-not $script:work) {
        $script:work = Join-Path ([IO.Path]::GetTempPath()) "addiem-ort-$PID"
        New-Item -ItemType Directory -Force $script:work | Out-Null
        $zip = Join-Path $script:work "ort.zip"
        Write-Host "받는 중 $OrtZip"
        Invoke-WebRequest -UseBasicParsing -Uri $OrtZip -OutFile $zip
        Check $zip $OrtZipSha
        Expand-Archive -LiteralPath $zip -DestinationPath $script:work -Force
    }
    Join-Path $script:work $inner
}

try {
    foreach ($f in $Files) {
        $to, $local, $remote, $sha = $f
        $target = Join-Path $Dest $to
        if ((Test-Path -LiteralPath $target) -and ((Sha $target) -eq $sha)) { Write-Host "있음 $to"; continue }
        New-Item -ItemType Directory -Force (Split-Path $target) | Out-Null
        $from = Join-Path $Source $local
        if (Test-Path -LiteralPath $from) {
            Check $from $sha
            Copy-Item -LiteralPath $from -Destination $target -Force
            Write-Host "복사 $to"
        } elseif ($Download) {
            if ($remote.StartsWith("hf:")) {
                $url = "$HfBase/$($remote.Substring(3))"
                Write-Host "받는 중 $url"
                Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile "$target.part"
                Check "$target.part" $sha
                Move-Item -LiteralPath "$target.part" -Destination $target -Force
            } else {
                $x = OrtFile $remote.Substring(4)
                Check $x $sha
                Copy-Item -LiteralPath $x -Destination $target -Force
            }
            Write-Host "받음 $to"
        } else {
            throw "없다: $from`n받아 둔 폴더를 -Source 로 알려 주거나 -Download 로 받는다"
        }
    }
    Write-Host "완료: $((Resolve-Path $Dest).Path)"
} finally {
    if ($work) { Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue }
}
