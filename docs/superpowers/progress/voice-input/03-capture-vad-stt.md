# Capture, VAD, and transcription

Status: **implementing**

## Implemented

- Added CPAL default-input capture for common integer and floating sample
  formats.
- Downmixed channels in the callback and handed mono samples to a wait-free
  SPSC ring buffer.
- Added a stateful linear resampler to 16 kHz mono.
- Added level calculation and approximately 10 Hz level events.
- Added embedded Silero VAD initialization and 512-sample endpoint processing.
- Added 800 ms configurable sentence segmentation while keeping capture open,
  a 30-second idle timeout, and a configurable 10-minute session cap.
- Added reusable Whisper context, blocking transcription worker, optional
  language pinning, timestamped transcript segments, and partial capability
  benchmark.
- Added timestamp-capable transcripts, bounded 12-second rolling re-transcription,
  committed-suffix overlap alignment, LocalAgreement-style prefixes, stale-result
  suppression, and one partial job in flight.
- Added distinct permission, no-audio, no-speech, and engine errors plus a
  three-failure circuit breaker.
- Added immediate cancellation that drops capture/initialization tasks and a
  Whisper abort callback so in-flight native decoding stops during CLI shutdown.

## Pending

- Complete a live microphone pass on macOS.
- Run real English and Chinese Whisper fixture/live passes.
- Validate permission-denied and input-device-loss mappings on macOS and Linux.
- Validate 12-second rolling-window rollover with long live speech and add a
  deterministic scripted rollover test.
- Add ignored real-model integration coverage with a redistributable WAV
  fixture.
