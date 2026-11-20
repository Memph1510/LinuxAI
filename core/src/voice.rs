//! Voice: talk to the OS and hear it answer.
//!
//! The model works in text and sees pictures; this organ is the thin layer around it: your speech becomes text,
//! and its answer becomes speech. Push to talk: press Enter to start, Enter again to stop (a key on the laptop, a
//! button on the phone later). Everything runs on the machine by default (private, works offline), and any part
//! can be swapped for another program or a cloud service by setting a command.
//!
//!   linuxai-core voice caps                    what this machine has for listening and speaking
//!   linuxai-core voice listen [--max SECS]     record until Enter (or SECS), print {"ok", "text"}
//!   linuxai-core voice transcribe FILE.wav     speech in a file to text
//!   linuxai-core voice say                     speak the text on stdin
//!
//! Defaults, found automatically:
//!   recording       pw-record (PipeWire), else arecord (ALSA)       or LINUXAI_RECORD_CMD (with {out})
//!   speech to text  whisper.cpp (whisper-cli) + LINUXAI_WHISPER_MODEL or LINUXAI_STT_CMD (JSON {"audio"} on stdin, prints text)
//!   text to speech  Piper + LINUXAI_PIPER_VOICE, else espeak-ng     or LINUXAI_TTS_CMD (text on stdin; plays it)

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn which(name: &str) -> Option<String> {
    std::env::var("PATH").ok()?.split(':').map(|d| PathBuf::from(d).join(name)).find(|p| p.is_file())
        .map(|p| p.to_string_lossy().to_string())
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

/// A job's model from the core's table (which includes the environment and the default), if the core answers.
fn from_table(job: &str) -> Option<String> {
    let client = crate::client::Client::new(crate::default_socket());
    crate::models::ask(&client, job).map(|v| v.strip_prefix("~/").map(|r| home().join(r).to_string_lossy().to_string()).unwrap_or(v))
}

fn piper_voice() -> Option<String> {
    from_table("speaking").or_else(|| env("LINUXAI_PIPER_VOICE")).filter(|v| v.ends_with(".onnx"))
}

fn whisper_model() -> PathBuf {
    from_table("speech_to_text").or_else(|| env("LINUXAI_WHISPER_MODEL")).map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share/linuxai/models/ggml-base.en.bin"))
}

/// The recorder command for a 16 kHz mono WAV file at `out`.
fn recorder(out: &Path) -> Option<Vec<String>> {
    let o = out.display().to_string();
    if let Some(c) = env("LINUXAI_RECORD_CMD") {
        return Some(vec!["sh".into(), "-c".into(), format!("exec {}", c.replace("{out}", &shell_quote(&o)))]);
    }
    if let Some(p) = which("pw-record") {
        return Some(vec![p, "--rate".into(), "16000".into(), "--channels".into(), "1".into(), "--format".into(), "s16".into(), o]);
    }
    which("arecord").map(|a| vec![a, "-q".into(), "-f".into(), "S16_LE".into(), "-r".into(), "16000".into(), "-c".into(), "1".into(), o])
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn caps() -> Value {
    let stt = if env("LINUXAI_STT_CMD").is_some() {
        json!("command (LINUXAI_STT_CMD)")
    } else if which("whisper-cli").is_some() {
        if whisper_model().exists() { json!("whisper.cpp") } else { json!(format!("whisper.cpp, but no model at {}", whisper_model().display())) }
    } else {
        Value::Null
    };
    let tts = if env("LINUXAI_TTS_CMD").is_some() {
        json!("command (LINUXAI_TTS_CMD)")
    } else if which("piper").is_some() && piper_voice().is_some() {
        json!("piper")
    } else if which("espeak-ng").is_some() {
        json!("espeak-ng")
    } else {
        Value::Null
    };
    let rec = recorder(Path::new("/tmp/x.wav")).map(|c| json!(Path::new(&c[0]).file_name().map(|f| f.to_string_lossy().to_string())));
    let mut advice = Vec::new();
    if rec.is_none() {
        advice.push("No recorder: install pipewire (pw-record) or alsa-utils (arecord).");
    }
    if stt.is_null() {
        advice.push("Nothing turns speech into text yet: install whisper.cpp and a model (base.en is small and good), or set LINUXAI_STT_CMD.");
    }
    if tts.is_null() {
        advice.push("Nothing speaks yet: install piper (natural voice) or espeak-ng (tiny), or set LINUXAI_TTS_CMD.");
    }
    json!({"ok": true, "record": rec, "speech_to_text": stt, "text_to_speech": tts, "advice": advice.join(" ")})
}

/// Start recording; stop it with `stop_recording`.
pub fn start_recording(out: &Path) -> Result<Child, String> {
    let cmd = recorder(out).ok_or("no recorder (pw-record or arecord) on this machine")?;
    Command::new(&cmd[0]).args(&cmd[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().map_err(|e| format!("couldn't start recording: {e}"))
}

/// Stop the way recorders expect (Ctrl-C), so they finish the file properly.
pub fn stop_recording(mut child: Child) {
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub fn transcribe(audio: &Path) -> Result<String, String> {
    if !audio.is_file() || std::fs::metadata(audio).map(|m| m.len() < 100).unwrap_or(true) {
        return Err("nothing was recorded".into());
    }
    let out = if let Some(c) = env("LINUXAI_STT_CMD") {
        let mut child = Command::new("sh").args(["-c", &c]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().map_err(|e| e.to_string())?;
        if let Some(mut i) = child.stdin.take() {
            let _ = i.write_all(json!({"audio": audio}).to_string().as_bytes());
        }
        child.wait_with_output().map_err(|e| e.to_string())?
    } else {
        let w = which("whisper-cli").ok_or("nothing turns speech into text yet (install whisper.cpp, or set LINUXAI_STT_CMD)")?;
        let model = whisper_model();
        if !model.exists() {
            return Err(format!("whisper.cpp needs a model at {} (or set LINUXAI_WHISPER_MODEL)", model.display()));
        }
        Command::new(w).arg("-m").arg(&model).arg("-f").arg(audio).args(["-nt", "-np"]).output().map_err(|e| e.to_string())?
    };
    if !out.status.success() {
        return Err(format!("speech to text failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let raw = String::from_utf8_lossy(&out.stdout).to_string();
    let text = serde_json::from_str::<Value>(&raw).ok().and_then(|v| v["text"].as_str().map(String::from)).unwrap_or(raw);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // whisper's markers for silence and noise aren't words
    let text = text.replace("[BLANK_AUDIO]", "").replace("(silence)", "").trim().to_string();
    if text.is_empty() {
        return Err("didn't catch anything".into());
    }
    Ok(text)
}

/// Make a written answer sound right out loud: no markdown, no code, links named not read.
pub fn speakable(text: &str) -> String {
    let mut out = Vec::new();
    let mut in_code = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || l.starts_with('|') {
            continue; // code and tables are for reading on screen
        }
        let mut s = l.trim_start_matches(['#', '>', '-', '*', ' ']).to_string();
        for m in ["**", "__", "`", "*"] {
            s = s.replace(m, "");
        }
        // [words](link) → words; bare links → "a link"
        while let (Some(a), Some(b)) = (s.find("]("), s.find(')')) {
            if b < a {
                break;
            }
            let open = s[..a].rfind('[').unwrap_or(a);
            s = format!("{}{}{}", &s[..open], &s[open + 1..a], &s[b + 1..]);
        }
        let s: Vec<String> = s.split_whitespace().map(|w| if w.starts_with("http") { "a link".into() } else { w.to_string() }).collect();
        if !s.is_empty() {
            out.push(s.join(" "));
        }
    }
    out.join(" ")
}

pub fn say(text: &str) -> Result<(), String> {
    let text = speakable(text);
    if text.is_empty() {
        return Ok(());
    }
    let run = |mut cmd: Command, input: &str| -> Result<(), String> {
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
        if let Some(mut i) = child.stdin.take() {
            let _ = i.write_all(input.as_bytes());
        }
        child.wait().map_err(|e| e.to_string()).map(|_| ())
    };
    if let Some(c) = env("LINUXAI_TTS_CMD") {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &c]);
        return run(cmd, &text);
    }
    if let (Some(piper), Some(voice)) = (which("piper"), piper_voice()) {
        let wav = std::env::temp_dir().join(format!("linuxai-say-{}.wav", std::process::id()));
        let mut cmd = Command::new(piper);
        cmd.arg("--model").arg(voice).arg("--output_file").arg(&wav);
        run(cmd, &text)?;
        let player = which("pw-play").or_else(|| which("aplay")).ok_or("no audio player (pw-play or aplay)")?;
        let r = Command::new(player).arg(&wav).stdout(Stdio::null()).stderr(Stdio::null()).status();
        let _ = std::fs::remove_file(&wav);
        return r.map(|_| ()).map_err(|e| e.to_string());
    }
    if let Some(e) = which("espeak-ng") {
        let mut cmd = Command::new(e);
        cmd.args(["-s", "170", "--stdin"]);
        return run(cmd, &text);
    }
    Err("nothing speaks yet (install piper or espeak-ng, or set LINUXAI_TTS_CMD)".into())
}

/// Push to talk: record until Enter on stdin (or `max` seconds), then transcribe.
pub fn listen(max: Duration) -> Value {
    let wav = std::env::temp_dir().join(format!("linuxai-listen-{}.wav", std::process::id()));
    let child = match start_recording(&wav) {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    eprintln!("listening… press Enter when you're done");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        let _ = tx.send(());
    });
    let started = Instant::now();
    let _ = rx.recv_timeout(max);
    // A press shorter than this isn't speech; give the recorder time to start properly.
    if let Some(rest) = Duration::from_millis(600).checked_sub(started.elapsed()) {
        std::thread::sleep(rest);
    }
    stop_recording(child);
    let r = match transcribe(&wav) {
        Ok(text) => json!({"ok": true, "text": text}),
        Err(e) => json!({"ok": false, "error": e}),
    };
    let _ = std::fs::remove_file(&wav);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV: Mutex<()> = Mutex::new(());

    #[test]
    fn answers_are_made_speakable() {
        let t = "## Two options\n\n**Ridgeway LX** is $34,900 — see [the listing](https://x.com/a).\n```\ncode\n```\n| a | b |\n- Summit Trail: https://y.com";
        assert_eq!(speakable(t), "Two options Ridgeway LX is $34,900 — see the listing. Summit Trail: a link");
    }

    #[test]
    fn record_transcribe_and_speak_through_configured_commands() {
        let _g = ENV.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("lx-voice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A stand-in microphone that writes a file and keeps "recording" until stopped.
        std::env::set_var("LINUXAI_RECORD_CMD", "head -c 4000 /dev/zero > {out}; sleep 30");
        std::env::set_var("LINUXAI_STT_CMD", format!("cat > {}/stt-job.json; echo '{{\"text\": \"  what is   this one \"}}'", dir.display()));
        std::env::set_var("LINUXAI_TTS_CMD", format!("cat > {}/said.txt", dir.display()));

        let wav = dir.join("a.wav");
        let child = start_recording(&wav).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let t = Instant::now();
        stop_recording(child);
        assert!(t.elapsed() < Duration::from_secs(2), "Ctrl-C stops the recorder promptly");
        assert_eq!(transcribe(&wav).unwrap(), "what is this one");
        let job: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("stt-job.json")).unwrap()).unwrap();
        assert_eq!(job["audio"], wav.display().to_string());

        say("**That one** is the Summit Trail: [specs](https://example.com).").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("said.txt")).unwrap(), "That one is the Summit Trail: specs.");

        // Nothing recorded, nothing heard.
        assert!(transcribe(&dir.join("missing.wav")).is_err());
        std::env::set_var("LINUXAI_STT_CMD", "cat >/dev/null; echo '[BLANK_AUDIO]'");
        assert_eq!(transcribe(&wav).unwrap_err(), "didn't catch anything");
        for k in ["LINUXAI_RECORD_CMD", "LINUXAI_STT_CMD", "LINUXAI_TTS_CMD"] {
            std::env::remove_var(k);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
