//! Text-to-speech platform services: voices, speech synthesis and playback control (M12).
//!
//! Bridges to the host operating system's native speech synthesizer:
//! - Windows: Native WinRT `SpeechSynthesizer` and `MediaPlayer` (pure safe Rust, zero temp scripts);
//! - macOS: `/usr/bin/say` with argument delimiter (`--`) to prevent option injection;
//! - Linux / BSD: `spd-say` (speech-dispatcher) or `espeak` with argument delimiter (`--`);
//! - Mock/Oracle mode: record spoken utterances for deterministic headless testing and CI.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// A text-to-speech voice available on this platform.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub language: String,
    pub gender: Option<String>,
}

/// Parameters for speech synthesis.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpeechOptions {
    /// Preferred voice name or identifier, or None for the system default.
    pub voice: Option<String>,
    /// Speech rate multiplier (1.0 is normal speed, 0.5 is half speed, 2.0 is double speed).
    pub rate: f32,
    /// Pitch multiplier (1.0 is normal pitch).
    pub pitch: f32,
    /// Volume from 0.0 (silent) to 1.0 (full volume).
    pub volume: f32,
}

impl Default for SpeechOptions {
    fn default() -> Self {
        Self { voice: None, rate: 1.0, pitch: 1.0, volume: 1.0 }
    }
}

/// The current playback status of speech synthesis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SpeechStatus {
    Idle,
    Speaking,
    Paused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TtsError {
    Unavailable(String),
    Failed(String),
}

impl std::fmt::Display for TtsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "text-to-speech unavailable: {m}"),
            Self::Failed(m) => write!(f, "speech synthesis failed: {m}"),
        }
    }
}

impl std::error::Error for TtsError {}

#[cfg(windows)]
struct WindowsTtsPlayer {
    player: windows::Media::Playback::MediaPlayer,
    synthesizer: windows::Media::SpeechSynthesis::SpeechSynthesizer,
    ended: std::sync::Arc<Mutex<bool>>,
    failure: std::sync::Arc<Mutex<Option<String>>>,
    stream: Option<windows::Media::SpeechSynthesis::SpeechSynthesisStream>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveBackend {
    #[cfg(windows)]
    WinRt,
    #[cfg(target_os = "macos")]
    Say,
    #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
    SpdSay,
    #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
    Espeak,
    Mock,
}

struct ActiveSpeech {
    status: SpeechStatus,
    current_text: Option<String>,
    options: SpeechOptions,
    generation: u64,
    backend: Option<ActiveBackend>,
    #[cfg(windows)]
    win_player: Option<WindowsTtsPlayer>,
    #[cfg(unix)]
    child: Option<std::process::Child>,
}

static MOCK_ENABLED: AtomicBool = AtomicBool::new(false);
static MOCK_SPOKEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn state() -> &'static Mutex<ActiveSpeech> {
    static STATE: OnceLock<Mutex<ActiveSpeech>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(ActiveSpeech {
            status: SpeechStatus::Idle,
            current_text: None,
            options: SpeechOptions::default(),
            generation: 0,
            backend: None,
            #[cfg(windows)]
            win_player: None,
            #[cfg(unix)]
            child: None,
        })
    })
}

/// Enable or disable mock TTS mode for deterministic testing without external processes.
pub fn set_mock(mock: bool) {
    MOCK_ENABLED.store(mock, Ordering::SeqCst);
}

/// Whether mock TTS mode is currently active.
pub fn is_mock() -> bool {
    MOCK_ENABLED.load(Ordering::SeqCst) || std::env::var_os("PDFCRAFT_TTS_MOCK").is_some()
}

/// Take all utterances spoken in mock mode since the last call.
pub fn take_mock_spoken() -> Vec<String> {
    let mut guard = match MOCK_SPOKEN.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    std::mem::take(&mut *guard)
}

/// Clear mock utterances.
pub fn clear_mock_spoken() {
    let _ = take_mock_spoken();
}

/// Default fallback voices returned when the platform reports none or in mock mode.
pub fn default_mock_voices() -> Vec<Voice> {
    vec![
        Voice { id: "system-default".into(), name: "System Default Voice".into(), language: "en-US".into(), gender: Some("Neutral".into()) },
        Voice { id: "pt-br-default".into(), name: "Voz Português (Brasil)".into(), language: "pt-BR".into(), gender: Some("Female".into()) },
    ]
}

/// List the text-to-speech voices available on this operating system.
pub fn list_voices() -> Vec<Voice> {
    if is_mock() {
        return default_mock_voices();
    }

    #[cfg(windows)]
    {
        list_windows_voices()
    }
    #[cfg(target_os = "macos")]
    {
        list_macos_voices()
    }
    #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
    {
        list_linux_voices()
    }
    #[cfg(target_arch = "wasm32")]
    {
        default_mock_voices()
    }
}

/// Current playback state of speech synthesis.
pub fn status() -> SpeechStatus {
    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    #[cfg(windows)]
    {
        if let Some(win) = guard.win_player.as_mut() {
            let ended = win.ended.lock().map(|d| *d).unwrap_or(false);
            let failed = win.failure.lock().map(|f| f.is_some()).unwrap_or(false);
            if ended || failed {
                let _ = win.player.Pause();
                guard.win_player = None;
                guard.status = SpeechStatus::Idle;
                guard.current_text = None;
            }
        }
    }
    #[cfg(unix)]
    {
        if let Some(child) = guard.child.as_mut() {
            if let Ok(Some(_)) = child.try_wait() {
                guard.child = None;
                guard.status = SpeechStatus::Idle;
                guard.current_text = None;
            }
        }
    }
    guard.status
}

/// The text currently being spoken, if any.
pub fn current_text() -> Option<String> {
    let guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    guard.current_text.clone()
}

/// Speak `text` using `options`. If speech is already playing, it is stopped first.
pub fn speak(text: &str, options: &SpeechOptions) -> Result<(), TtsError> {
    stop()?;

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    if is_mock() {
        let mut mock_guard = match MOCK_SPOKEN.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        mock_guard.push(trimmed.to_string());

        let mut s = match state().lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        s.generation = s.generation.wrapping_add(1);
        s.status = SpeechStatus::Speaking;
        s.current_text = Some(trimmed.to_string());
        s.options = options.clone();
        s.backend = Some(ActiveBackend::Mock);
        return Ok(());
    }

    #[cfg(windows)]
    {
        speak_windows(trimmed, options)
    }
    #[cfg(target_os = "macos")]
    {
        speak_macos(trimmed, options)
    }
    #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
    {
        speak_linux(trimmed, options)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let mut s = match state().lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        s.status = SpeechStatus::Speaking;
        s.current_text = Some(trimmed.to_string());
        s.options = options.clone();
        Ok(())
    }
}

/// Pause current speech playback.
pub fn pause() -> Result<(), TtsError> {
    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.status == SpeechStatus::Speaking {
        #[cfg(windows)]
        {
            if let Some(win) = guard.win_player.as_ref() {
                let _ = win.player.Pause();
            }
        }
        #[cfg(unix)]
        {
            if let Some(child) = guard.child.as_ref() {
                let _ = std::process::Command::new("kill")
                    .args(["-STOP", &child.id().to_string()])
                    .output();
            }
        }
        guard.status = SpeechStatus::Paused;
    }
    Ok(())
}

/// Resume speech playback if paused.
pub fn resume() -> Result<(), TtsError> {
    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.status == SpeechStatus::Paused {
        #[cfg(windows)]
        {
            if let Some(win) = guard.win_player.as_ref() {
                let _ = win.player.Play();
            }
        }
        #[cfg(unix)]
        {
            if let Some(child) = guard.child.as_ref() {
                let _ = std::process::Command::new("kill")
                    .args(["-CONT", &child.id().to_string()])
                    .output();
            }
        }
        guard.status = SpeechStatus::Speaking;
    }
    Ok(())
}

/// Stop speech playback and reset playback state to idle.
pub fn stop() -> Result<(), TtsError> {
    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    guard.generation = guard.generation.wrapping_add(1);

    #[cfg(windows)]
    {
        if let Some(mut win) = guard.win_player.take() {
            let _ = win.player.Pause();
            let _ = win.player.SetSource(None::<&windows::Media::Playback::IMediaPlaybackSource>);
            if let Some(stream) = win.stream.take() {
                let _ = stream.Close();
            }
            let _ = win.player.Close();
            let _ = win.synthesizer.Close();
        }
    }

    #[cfg(unix)]
    {
        if let Some(mut child) = guard.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    guard.backend = None;
    guard.status = SpeechStatus::Idle;
    guard.current_text = None;
    Ok(())
}

// ---------------------------------------------------------------------------
// Windows implementation via pure safe WinRT (SpeechSynthesizer + MediaPlayer)
// ---------------------------------------------------------------------------

#[cfg(windows)]
/// Enumerate voices installed on Windows using WinRT SpeechSynthesizer.
fn list_windows_voices() -> Vec<Voice> {
    use windows::Media::SpeechSynthesis::SpeechSynthesizer;
    let voices = match SpeechSynthesizer::AllVoices() {
        Ok(v) => v,
        Err(_) => return default_mock_voices(),
    };
    let n = voices.Size().unwrap_or(0).min(256);
    let mut list = Vec::new();
    for i in 0..n {
        if let Ok(v) = voices.GetAt(i) {
            let id = v.Id().map(|s| s.to_string_lossy()).unwrap_or_default();
            let name = v.DisplayName().map(|s| s.to_string_lossy()).unwrap_or_default();
            let lang = v.Language().map(|s| s.to_string_lossy()).unwrap_or_default();
            let gender = v.Gender().ok().map(|g| match g.0 {
                0 => "Female".to_string(),
                1 => "Male".to_string(),
                _ => "Neutral".to_string(),
            });
            if !name.is_empty() {
                list.push(Voice {
                    id: if id.is_empty() { name.clone() } else { id },
                    name,
                    language: lang,
                    gender,
                });
            }
        }
    }
    if list.is_empty() {
        default_mock_voices()
    } else {
        list
    }
}

#[cfg(windows)]
/// Synthesize and play speech on Windows via in-memory WinRT stream on a background worker thread.
fn speak_windows(text: &str, options: &SpeechOptions) -> Result<(), TtsError> {
    use windows::{
        core::HSTRING,
        Foundation::TypedEventHandler,
        Media::{
            Core::MediaSource,
            Playback::MediaPlayer,
            SpeechSynthesis::SpeechSynthesizer,
        },
    };
    use windows_future::AsyncStatus;

    let synthesizer = SpeechSynthesizer::new().map_err(|e| TtsError::Failed(format!("Windows SpeechSynthesizer: {e}")))?;
    let player = MediaPlayer::new().map_err(|e| TtsError::Failed(format!("Windows MediaPlayer: {e}")))?;
    player.SetAutoPlay(false).map_err(|e| TtsError::Failed(format!("MediaPlayer: {e}")))?;
    if let Ok(m) = player.CommandManager() {
        let _ = m.SetIsEnabled(false);
    }

    let ended = std::sync::Arc::new(Mutex::new(false));
    let end = ended.clone();
    player
        .MediaEnded(&TypedEventHandler::new(move |_, _| {
            if let Ok(mut done) = end.lock() {
                *done = true;
            }
            Ok(())
        }))
        .map_err(|e| TtsError::Failed(format!("MediaEnded handler: {e}")))?;

    let failure = std::sync::Arc::new(Mutex::new(None));
    let fail = failure.clone();
    player
        .MediaFailed(&TypedEventHandler::<MediaPlayer, windows::Media::Playback::MediaPlayerFailedEventArgs>::new(move |_, args| {
            if let Ok(mut message) = fail.lock() {
                *message = Some(
                    args.as_ref()
                        .and_then(|a| a.ErrorMessage().ok())
                        .map_or_else(|| "Windows audio playback failed".into(), |s| s.to_string_lossy()),
                );
            }
            Ok(())
        }))
        .map_err(|e| TtsError::Failed(format!("MediaFailed handler: {e}")))?;

    if let Some(id) = &options.voice {
        if let Ok(voices) = SpeechSynthesizer::AllVoices() {
            let n = voices.Size().unwrap_or(0).min(256);
            for i in 0..n {
                if let Ok(v) = voices.GetAt(i) {
                    let voice_id = v.Id().map(|s| s.to_string_lossy()).unwrap_or_default();
                    let voice_name = v.DisplayName().map(|s| s.to_string_lossy()).unwrap_or_default();
                    if voice_id == *id || voice_name == *id {
                        let _ = synthesizer.SetVoice(&v);
                        break;
                    }
                }
            }
        }
    }

    if let Ok(opts) = synthesizer.Options() {
        let _ = opts.SetSpeakingRate(options.rate.clamp(0.5, 3.0) as f64);
    }
    let _ = player.SetVolume(options.volume.clamp(0.0, 1.0) as f64);

    let op = synthesizer
        .SynthesizeTextToStreamAsync(&HSTRING::from(text))
        .map_err(|e| TtsError::Failed(format!("synthesize text: {e}")))?;

    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    guard.generation = guard.generation.wrapping_add(1);
    let current_gen = guard.generation;
    guard.status = SpeechStatus::Speaking;
    guard.current_text = Some(text.to_string());
    guard.options = options.clone();
    guard.backend = Some(ActiveBackend::WinRt);
    guard.win_player = None;
    drop(guard);

    let thread_res = std::thread::Builder::new()
        .name("pdfcraft-winrt-tts".into())
        .spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            let fail = || {
                let mut guard = match state().lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                if guard.generation == current_gen {
                    guard.status = SpeechStatus::Idle;
                    guard.current_text = None;
                }
            };

            loop {
                {
                    let guard = match state().lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    if guard.generation != current_gen {
                        let _ = op.Cancel();
                        return;
                    }
                }

                match op.Status() {
                    Ok(AsyncStatus::Completed) => break,
                    Ok(AsyncStatus::Started) => {
                        if std::time::Instant::now() > deadline {
                            let _ = op.Cancel();
                            fail();
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(15));
                    }
                    _ => {
                        fail();
                        return;
                    }
                }
            }

            let stream = match op.GetResults() {
                Ok(s) => s,
                Err(_) => {
                    fail();
                    return;
                }
            };
            let content_type = match stream.ContentType() {
                Ok(ct) => ct,
                Err(_) => return fail(),
            };
            let source = match MediaSource::CreateFromStream(&stream, &content_type) {
                Ok(s) => s,
                Err(_) => return fail(),
            };
            if player.SetSource(&source).is_err() {
                fail();
                return;
            }

            let mut guard = match state().lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if guard.generation == current_gen {
                if guard.status == SpeechStatus::Paused {
                    let _ = player.Pause();
                } else if player.Play().is_err() {
                    guard.status = SpeechStatus::Idle;
                    guard.current_text = None;
                    return;
                }
                guard.win_player = Some(WindowsTtsPlayer {
                    player,
                    synthesizer,
                    ended,
                    failure,
                    stream: Some(stream),
                });
            }
        });

    if let Err(e) = thread_res {
        let mut guard = match state().lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard.generation == current_gen {
            guard.status = SpeechStatus::Idle;
            guard.current_text = None;
        }
        return Err(TtsError::Failed(format!("worker thread spawn: {e}")));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// macOS implementation via /usr/bin/say
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
/// Enumerate available macOS speech voices using the say CLI.
fn list_macos_voices() -> Vec<Voice> {
    let output = match std::process::Command::new("say").arg("-v").arg("?").output() {
        Ok(out) if out.status.success() => out.stdout,
        _ => return default_mock_voices(),
    };

    let text = String::from_utf8_lossy(&output);
    let mut voices = Vec::new();
    for line in text.lines() {
        let head = line.split('#').next().unwrap_or("").trim_end();
        if let Some((name, lang)) = head.rsplit_once(char::is_whitespace) {
            let name = name.trim();
            if !name.is_empty() {
                voices.push(Voice {
                    id: name.to_string(),
                    name: name.to_string(),
                    language: lang.replace('_', "-"),
                    gender: None,
                });
            }
        }
    }

    if voices.is_empty() { default_mock_voices() } else { voices }
}

#[cfg(target_os = "macos")]
/// Build macOS /usr/bin/say command reading text from standard input (-f -).
fn build_say_command(options: &SpeechOptions) -> std::process::Command {
    let mut cmd = std::process::Command::new("say");
    if let Some(v) = &options.voice {
        cmd.arg("-v").arg(v);
    }
    let wpm = (175.0 * options.rate.clamp(0.5, 3.0)).round() as u32;
    cmd.arg("-r").arg(wpm.to_string());
    cmd.arg("-f").arg("-");
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    cmd
}

#[cfg(target_os = "macos")]
/// Speak text on macOS using /usr/bin/say with text delivered via stdin asynchronously.
fn speak_macos(text: &str, options: &SpeechOptions) -> Result<(), TtsError> {
    use std::io::Write;
    let mut cmd = build_say_command(options);
    let mut child = cmd.spawn().map_err(|e| TtsError::Failed(format!("could not spawn say process: {e}")))?;
    let child_stdin = child.stdin.take();
    let text_owned = text.to_string();
    let _ = std::thread::Builder::new()
        .name("pdfcraft-tts-stdin".into())
        .spawn(move || {
            if let Some(mut stdin) = child_stdin {
                let _ = stdin.write_all(text_owned.as_bytes());
            }
        });

    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    guard.generation = guard.generation.wrapping_add(1);
    guard.status = SpeechStatus::Speaking;
    guard.current_text = Some(text.to_string());
    guard.options = options.clone();
    guard.backend = Some(ActiveBackend::Say);
    guard.child = Some(child);

    Ok(())
}

// ---------------------------------------------------------------------------
// Linux / BSD implementation via spd-say or espeak
// ---------------------------------------------------------------------------

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
/// Enumerate available Linux speech voices.
fn list_linux_voices() -> Vec<Voice> {
    let output = match std::process::Command::new("spd-say").arg("-L").output() {
        Ok(out) if out.status.success() => out.stdout,
        _ => return Vec::new(),
    };

    let text = String::from_utf8_lossy(&output);
    let mut voices = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || (idx == 0 && line.to_uppercase().starts_with("NAME")) {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if !parts.is_empty() {
            let name = parts[0].to_string();
            let lang = parts.get(1).map(|s| s.replace('_', "-")).unwrap_or_else(|| "en".to_string());
            voices.push(Voice { id: name.clone(), name, language: lang, gender: None });
        }
    }
    voices
}

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
/// Build Linux spd-say command with -w flag, reading text from standard input (-e), and silenced outputs.
fn build_spd_say_command(options: &SpeechOptions) -> std::process::Command {
    let mut cmd = std::process::Command::new("spd-say");
    cmd.arg("-w");
    let rate_pct = (((options.rate.clamp(0.5, 2.0) - 1.0) * 100.0).round() as i32).clamp(-100, 100);
    let pitch_pct = (((options.pitch.clamp(0.5, 2.0) - 1.0) * 100.0).round() as i32).clamp(-100, 100);
    cmd.arg("-r").arg(rate_pct.to_string());
    cmd.arg("-p").arg(pitch_pct.to_string());
    if let Some(v) = &options.voice {
        cmd.arg("-y").arg(v);
    }
    cmd.arg("-e");
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    cmd
}

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
/// Build Linux espeak fallback command reading text from standard input (--stdin) with silenced outputs.
fn build_espeak_command() -> std::process::Command {
    let mut espeak = std::process::Command::new("espeak");
    espeak.arg("--stdin");
    espeak.stdin(std::process::Stdio::piped());
    espeak.stdout(std::process::Stdio::null());
    espeak.stderr(std::process::Stdio::null());
    espeak
}

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
/// Speak text on Linux using speech-dispatcher (spd-say) or espeak fallback via async stdin.
fn speak_linux(text: &str, options: &SpeechOptions) -> Result<(), TtsError> {
    use std::io::Write;
    let mut cmd = build_spd_say_command(options);

    let (mut child, backend) = match cmd.spawn() {
        Ok(c) => (c, ActiveBackend::SpdSay),
        Err(_) => {
            let mut espeak = build_espeak_command();
            let c = espeak.spawn().map_err(|e| TtsError::Failed(format!("neither spd-say nor espeak available: {e}")))?;
            (c, ActiveBackend::Espeak)
        }
    };
    let child_stdin = child.stdin.take();
    let text_owned = text.to_string();
    let _ = std::thread::Builder::new()
        .name("pdfcraft-tts-stdin".into())
        .spawn(move || {
            if let Some(mut stdin) = child_stdin {
                let _ = stdin.write_all(text_owned.as_bytes());
            }
        });

    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    guard.generation = guard.generation.wrapping_add(1);
    guard.status = SpeechStatus::Speaking;
    guard.current_text = Some(text.to_string());
    guard.options = options.clone();
    guard.backend = Some(backend);
    guard.child = Some(child);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_tts_records_utterances_and_updates_status() {
        set_mock(true);
        clear_mock_spoken();

        assert_eq!(status(), SpeechStatus::Idle);
        assert_eq!(current_text(), None);

        let voices = list_voices();
        assert!(!voices.is_empty());

        let opts = SpeechOptions { voice: Some("pt-br-default".into()), rate: 1.2, pitch: 1.0, volume: 0.9 };

        speak("Olá mundo, lendo PDF em voz alta", &opts).unwrap();
        assert_eq!(status(), SpeechStatus::Speaking);
        assert_eq!(current_text(), Some("Olá mundo, lendo PDF em voz alta".into()));

        pause().unwrap();
        assert_eq!(status(), SpeechStatus::Paused);

        resume().unwrap();
        assert_eq!(status(), SpeechStatus::Speaking);

        stop().unwrap();
        assert_eq!(status(), SpeechStatus::Idle);
        assert_eq!(current_text(), None);

        let spoken = take_mock_spoken();
        assert_eq!(spoken, vec!["Olá mundo, lendo PDF em voz alta"]);

        set_mock(false);
    }

    #[test]
    fn command_builders_use_stdin_modes() {
        #[cfg(target_os = "macos")]
        {
            let cmd = build_say_command(&SpeechOptions::default());
            let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
            assert!(args.contains(&"-f".to_string()) && args.contains(&"-".to_string()));
        }

        #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
        {
            let opts = SpeechOptions { voice: Some("test-voice".into()), ..Default::default() };
            let cmd = build_spd_say_command(&opts);
            let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
            assert!(args.contains(&"-e".to_string()), "spd-say command must use -e for stdin");
            assert!(args.contains(&"-y".to_string()), "spd-say command must use -y for synthesis voice");
            assert!(args.contains(&"test-voice".to_string()), "spd-say command must pass selected voice");

            let espeak = build_espeak_command();
            let espeak_args: Vec<String> = espeak.get_args().map(|a| a.to_string_lossy().to_string()).collect();
            assert!(espeak_args.contains(&"--stdin".to_string()), "espeak command must use --stdin");
        }
    }
}
