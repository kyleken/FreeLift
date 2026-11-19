# Makes the spoken test commands used by voice::tests::recogniser_hears_recorded_speech,
# with Windows' built-in text-to-speech voice (16 kHz, mono, 16-bit WAV).
# Run from this folder:  powershell -ExecutionPolicy Bypass -File make_commands.ps1
Add-Type -AssemblyName System.Speech
$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
$phrases = [ordered]@{
    "weight_on.wav"         = "Weight on."
    "weight_off.wav"        = "Weight off."
    "stop.wav"              = "Stop!"
    "wake_set_twenty.wav"   = "Weight. Set weight to twenty."
}
foreach ($file in $phrases.Keys) {
    $s = New-Object System.Speech.Synthesis.SpeechSynthesizer
    $s.Rate = -1
    $s.SetOutputToWaveFile((Join-Path $PSScriptRoot $file), $format)
    $s.Speak($phrases[$file])
    $s.Dispose()
}
