//! Reach: when the AI is undecided it doesn't sit and wait; it reaches the human.
//!
//! Follows the core's events and sends a message whenever a plan needs the human (a question, an approval),
//! a follow-up is due, a task is blocked, or the AI has something to tell them. No AI model is involved, so
//! it works even when the AI is offline. Messages go out through LINUXAI_NOTIFY_CMD: any command that reads
//! the message on stdin (an SMS gateway, a push service), until the LinuxAI Android app exists.

use crate::client::Client;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

fn when(ts: f64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(ts as i64, 0).single() {
        Some(t) => {
            let today = Local::now().date_naive();
            let day = if t.date_naive() == today {
                String::new()
            } else if t.date_naive() == today.succ_opt().unwrap_or(today) {
                "tomorrow ".into()
            } else {
                t.format("%a %b %-d ").to_string()
            };
            format!("{day}{}", t.format("%-I:%M%P"))
        }
        None => String::new(),
    }
}

/// The text sent to the human for a core event, or None if it isn't worth a message.
pub fn message_for(ev: &Value) -> Option<String> {
    let kind = ev["kind"].as_str().unwrap_or("");
    let d = &ev["data"];
    let w = &d["wait"];
    match kind {
        "plan.waiting" | "plan.reminder" if w["question"].is_string() => {
            let mut lines = vec![format!("{}LinuxAI needs you: {}", if kind == "plan.reminder" { "Reminder: " } else { "" },
                                         w["question"].as_str().unwrap_or(""))];
            if let Some(why) = w["why"].as_str() {
                lines.push(format!("Why I'm unsure: {why}"));
            }
            if let Some(lean) = w["lean"].as_str() {
                lines.push(format!("I'd lean: {lean}"));
            }
            let opts: Vec<String> = w["opts"].as_array().map(|a| a.iter().filter_map(|o| o.as_str().map(String::from)).collect()).unwrap_or_default();
            if !opts.is_empty() {
                lines.push(format!("Reply: {}", opts.join(" / ")));
            }
            match (w["fallback"].as_str(), w["until"].as_f64()) {
                (Some(f), Some(u)) => lines.push(format!("If I don't hear back by {}, I'll go with: {f}", when(u))),
                (Some(f), None) => lines.push(format!("If I don't hear back, I'll go with: {f}")),
                _ => {}
            }
            lines.push(format!("[{}]", d["plan_id"].as_str().unwrap_or("")));
            Some(lines.join("\n"))
        }
        "ai.tell" => d["text"].as_str().filter(|t| !t.is_empty()).map(|t| format!("LinuxAI: {t}")),
        "task.failed" => Some(format!("LinuxAI: blocked on '{}': {}. I'm looking at what to do next and will check with you before trying anything new.",
                                      d["title"].as_str().unwrap_or(""), d["error"].as_str().unwrap_or("unknown"))),
        "calendar.soon" => Some(format!("LinuxAI: in {} min: {}{}", d["minutes"].as_i64().unwrap_or(0), d["title"].as_str().unwrap_or(""),
                                          d["where"].as_str().filter(|w| !w.is_empty()).map(|w| format!(" at {w}")).unwrap_or_default())),
        "calendar.invite_cancelled" => Some(format!("LinuxAI: \"{}\" was cancelled by the organizer; it's off your calendar.",
                                                    d["title"].as_str().unwrap_or(""))),
        "browser.handoff" => {
            let note = d["note"].as_str().filter(|n| !n.is_empty()).unwrap_or("I need you to take over here");
            Some(format!("LinuxAI: {note}. It's open in the browser: {} ({}). Give the tab back when you're done.",
                         d["title"].as_str().unwrap_or(""), d["url"].as_str().unwrap_or("")))
        }
        // A bug in LinuxAI itself (a caught panic, or the core restarting after a crash). One text per incident.
        "linuxai.crash" => Some(format!("LinuxAI: internal error in {} ({}): {}", d["component"].as_str().unwrap_or("?"),
                                        d["what"].as_str().unwrap_or("?"), d["message"].as_str().unwrap_or("unknown"))),
        // A plan stopped by an internal error is already reported as linuxai.crash.
        "plan.failed" if d["error"].as_str().map(|e| e.starts_with("LinuxAI hit an internal error")).unwrap_or(false) => None,
        "plan.failed" => Some(format!("LinuxAI: '{}' failed: {}. I'll look into it.", d["plan"].as_str().unwrap_or(""),
                                      d["error"].as_str().unwrap_or("unknown"))),
        _ => None,
    }
}

pub fn send(text: &str, cmd: &str) -> bool {
    if cmd.trim().is_empty() {
        return false;
    }
    let Ok(mut child) = Command::new("sh").args(["-c", cmd]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

pub fn run(client: Client) -> std::io::Result<()> {
    client.wait_until_up();
    let cmd = std::env::var("LINUXAI_NOTIFY_CMD").unwrap_or_default();
    eprintln!("linuxai reach: connected; channel: {}", if cmd.is_empty() { "session only (set LINUXAI_NOTIFY_CMD)" } else { "command" });
    // Anything already waiting gets one message, so nothing sits unseen.
    if let Ok(r) = client.request("waiting", json!({})) {
        for p in r["plans"].as_array().cloned().unwrap_or_default() {
            if let Some(t) = message_for(&json!({"kind": "plan.waiting", "data": p})) {
                send(&t, &cmd);
            }
        }
    }
    for ev in client.follow()? {
        if let Some(t) = message_for(&ev) {
            if !send(&t, &cmd) && !cmd.is_empty() {
                eprintln!("linuxai reach: could not send through LINUXAI_NOTIFY_CMD");
            }
        }
    }
    Ok(())
}
