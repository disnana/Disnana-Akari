param(
    [Parameter(Mandatory)]
    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
$speaker = [System.Speech.Synthesis.SpeechSynthesizer]::new()
try {
    $voice = $speaker.GetInstalledVoices() | Where-Object {
        $_.Enabled -and $_.VoiceInfo.Culture.Name -eq 'ja-JP'
    } | Select-Object -First 1
    if (!$voice) {
        throw 'Windows の日本語音声が見つかりません。'
    }
    $speaker.SelectVoice($voice.VoiceInfo.Name)
    $speaker.Volume = 65
    $speaker.SetOutputToWaveFile($OutputPath)
    $speaker.Speak('こんにちは。あかりの音声再生テストです。')
} finally {
    $speaker.Dispose()
}
