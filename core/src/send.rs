//! Outgoing email: the `send_email` verb. It acts on the outside world, so a plan that sends needs the human's OK
//! (and, when it rests on outside information, the separate outside trust), until they answer "always" for that
//! recipient. Sent over SMTP with curl, the same way incoming mail uses IMAP:
//!   LINUXAI_SMTP_URL       smtps://smtp.example.com:465 (or smtp://…:587, upgraded to TLS; TLS is required)
//!   LINUXAI_SMTP_USER / LINUXAI_SMTP_PASSWORD   (the password goes to curl on stdin, never on its command line)
//!   LINUXAI_MAIL_FROM      the address it's sent from (default: the user)
//! A reply (`in_reply_to`: the id of a stored message) threads under the original. Every message sent is kept.

use crate::memory::Memory;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

pub struct SmtpConfig {
    pub url: String,
    pub user: String,
    pub password: String,
    pub from: String,
    /// Off only for tests against a local stand-in server.
    pub require_tls: bool,
}

impl SmtpConfig {
    pub fn from_env() -> Result<SmtpConfig, String> {
        let env = |k: &str| std::env::var(format!("LINUXAI_{k}")).unwrap_or_default();
        let url = env("SMTP_URL");
        if url.is_empty() {
            return Err("sending email isn't set up: set LINUXAI_SMTP_URL, LINUXAI_SMTP_USER, LINUXAI_SMTP_PASSWORD \
                        and LINUXAI_MAIL_FROM".into());
        }
        let user = env("SMTP_USER");
        let from = Some(env("MAIL_FROM")).filter(|f| !f.is_empty()).unwrap_or_else(|| user.clone());
        Ok(SmtpConfig { url, user, password: env("SMTP_PASSWORD"), from, require_tls: true })
    }
}

/// One plain address: no display names, lists, spaces or line breaks (nothing that could smuggle in a header).
pub fn valid_address(a: &str) -> bool {
    let a = a.trim();
    let Some((local, domain)) = a.split_once('@') else { return false };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && a.len() <= 254
        && a.chars().all(|c| c.is_ascii_graphic() && !"<>,;:\"()[]\\".contains(c)) && !domain.contains('@')
}

fn encode_header(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
        s.to_string()
    } else {
        format!("=?UTF-8?B?{}?=", crate::compositor::b64(s.as_bytes()))
    }
}

fn wrap(s: &str, n: usize) -> String {
    s.as_bytes().chunks(n).map(|c| String::from_utf8_lossy(c).to_string()).collect::<Vec<_>>().join("\r\n")
}

/// The message, ready to send, and its Message-ID.
pub fn compose(from: &str, to: &[String], subject: &str, text: &str, reply_to_id: Option<&str>) -> (String, String) {
    let domain = from.split_once('@').map(|(_, d)| d).unwrap_or("linuxai.local");
    let mid = format!("<{}.{}@{}>", crate::memory::new_id("lx"), std::process::id(), domain);
    let mut h = vec![
        format!("From: {from}"),
        format!("To: {}", to.join(", ")),
        format!("Subject: {}", encode_header(subject)),
        format!("Date: {}", chrono::Local::now().to_rfc2822()),
        format!("Message-ID: {mid}"),
    ];
    if let Some(r) = reply_to_id.filter(|r| !r.is_empty()) {
        h.push(format!("In-Reply-To: {r}"));
        h.push(format!("References: {r}"));
    }
    h.extend(["MIME-Version: 1.0".into(), "Content-Type: text/plain; charset=utf-8".into(),
              "Content-Transfer-Encoding: base64".into(), "X-Mailer: LinuxAI".into()]);
    let body = text.replace("\r\n", "\n").replace('\n', "\r\n");
    (format!("{}\r\n\r\n{}\r\n", h.join("\r\n"), wrap(&crate::compositor::b64(body.as_bytes()), 76)), mid)
}

fn send_with_curl(cfg: &SmtpConfig, to: &[String], message: &str) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("linuxai-send-{}-{}", std::process::id(), crate::memory::new_id("")));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let file = dir.join("message.eml");
    std::fs::write(&file, message).map_err(|e| e.to_string())?;
    let mut args = vec!["-sS".to_string(), "--config".into(), "-".into(), "--url".into(), cfg.url.clone(),
                        "--mail-from".into(), cfg.from.clone(), "--upload-file".into(), file.display().to_string()];
    for t in to {
        args.extend(["--mail-rcpt".into(), t.clone()]);
    }
    if cfg.require_tls {
        args.push("--ssl-reqd".into());
    }
    let result = (|| {
        let mut child = Command::new("curl").args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().map_err(|e| format!("curl: {e}"))?;
        let creds = if cfg.user.is_empty() { String::new() } else {
            format!("user = \"{}:{}\"\n", cfg.user.replace('"', "\\\""), cfg.password.replace('"', "\\\""))
        };
        child.stdin.take().ok_or("no stdin")?.write_all(creds.as_bytes()).map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() { Ok(()) } else { Err(format!("sending failed: {}", String::from_utf8_lossy(&out.stderr).trim())) }
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Send one email (args: to, subject, text, in_reply_to?) and keep a copy.
pub fn send(mem: &Memory, cfg: &SmtpConfig, args: &Value) -> Result<Value, String> {
    let to: Vec<String> = match &args["to"] {
        Value::String(s) => vec![s.trim().to_string()],
        Value::Array(a) => a.iter().map(|v| crate::engine::display(v).trim().to_string()).collect(),
        _ => vec![],
    };
    if to.is_empty() || to.len() > 10 {
        return Err("to: one address, or a list of up to 10".into());
    }
    if let Some(bad) = to.iter().find(|a| !valid_address(a)) {
        return Err(format!("{bad:?} isn't a plain email address"));
    }
    if !valid_address(&cfg.from) {
        return Err(format!("the sending address {:?} isn't a plain email address (LINUXAI_MAIL_FROM)", cfg.from));
    }
    let subject = crate::engine::display(&args["subject"]);
    if subject.contains(['\r', '\n']) || subject.len() > 300 {
        return Err("the subject must be one line, at most 300 characters".into());
    }
    let text = crate::engine::display(&args["text"]);
    if text.trim().is_empty() {
        return Err("the email has no text".into());
    }
    // A reply threads under the message it answers (the original's Message-ID, from the core's own record of it).
    let original = args["in_reply_to"].as_str().filter(|s| !s.is_empty()).map(|id| mem.object(id).ok_or(format!("no stored message {id}")))
        .transpose()?;
    let reply_to_id = original.as_ref().and_then(|o| o["source"]["message_id"].as_str()).map(String::from)
        .filter(|m| m.starts_with('<') && m.ends_with('>') && !m.contains(['\r', '\n']));
    let (message, mid) = compose(&cfg.from, &to, &subject, &text, reply_to_id.as_deref());
    send_with_curl(cfg, &to, &message)?;
    let id = mem.put_object("sent_message", None, &json!({"to": to, "from": cfg.from, "subject": subject, "text": text,
        "in_reply_to": args["in_reply_to"], "message_id": mid, "sent": crate::memory::now(), "trust": "ours"}));
    Ok(json!({"sent": true, "id": id, "message_id": mid, "to": to}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    /// A stand-in mail server: speaks enough SMTP to accept one message, and hands back what it received.
    fn fake_smtp() -> (String, std::thread::JoinHandle<(Vec<String>, String)>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("smtp://127.0.0.1:{}", l.local_addr().unwrap().port());
        let h = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            let mut r = BufReader::new(s);
            let (mut cmds, mut data) = (vec![], String::new());
            w.write_all(b"220 fake ESMTP\r\n").unwrap();
            let mut line = String::new();
            loop {
                line.clear();
                if r.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let cmd = line.trim_end().to_string();
                let up = cmd.to_uppercase();
                cmds.push(cmd.clone());
                if up.starts_with("EHLO") {
                    w.write_all(b"250-fake\r\n250 8BITMIME\r\n").unwrap();
                } else if up.starts_with("DATA") {
                    w.write_all(b"354 go\r\n").unwrap();
                    loop {
                        line.clear();
                        r.read_line(&mut line).unwrap();
                        if line == ".\r\n" {
                            break;
                        }
                        data.push_str(&line);
                    }
                    w.write_all(b"250 queued\r\n").unwrap();
                } else if up.starts_with("QUIT") {
                    w.write_all(b"221 bye\r\n").unwrap();
                    break;
                } else {
                    w.write_all(b"250 ok\r\n").unwrap();
                }
            }
            (cmds, data)
        });
        (url, h)
    }

    #[test]
    fn a_reply_goes_out_threaded_and_is_kept() {
        let mem = Memory::in_memory();
        let orig = mem.put_object("message", None, &json!({"source": {"message_id": "<m1@hollis-logistics.com>", "from": "dana@hollis-logistics.com"}}));
        let (url, server) = fake_smtp();
        let cfg = SmtpConfig { url, user: String::new(), password: String::new(), from: "me@example.com".into(), require_tls: false };
        let r = send(&mem, &cfg, &json!({"to": "dana@hollis-logistics.com", "subject": "Re: HL-3100 — $3,500",
            "text": "Hi Dana,\nWe can do it at $3,500.\n.\nThanks", "in_reply_to": orig})).unwrap();
        let (cmds, data) = server.join().unwrap();
        assert!(cmds.iter().any(|c| c == "MAIL FROM:<me@example.com>"), "{cmds:?}");
        assert!(cmds.iter().any(|c| c == "RCPT TO:<dana@hollis-logistics.com>"), "{cmds:?}");
        assert!(data.contains("In-Reply-To: <m1@hollis-logistics.com>\r\n"), "{data}");
        assert!(data.contains("Subject: =?UTF-8?B?"), "non-ASCII subjects are encoded: {data}");
        let b64: String = data.split("\r\n\r\n").nth(1).unwrap().split("\r\n").collect();
        assert!(b64.starts_with(&crate::compositor::b64("Hi Dana,".as_bytes())[..8]), "{data}");
        let kept = mem.object(r["id"].as_str().unwrap()).unwrap();
        assert_eq!(kept["subject"], "Re: HL-3100 — $3,500");
        assert_eq!(kept["message_id"], r["message_id"]);
    }

    #[test]
    fn nothing_can_smuggle_in_a_header_or_a_recipient() {
        let mem = Memory::in_memory();
        let cfg = SmtpConfig { url: "smtp://127.0.0.1:9".into(), user: String::new(), password: String::new(),
                               from: "me@example.com".into(), require_tls: false };
        for (args, want) in [
            (json!({"to": "dana@x.com\r\nBcc: evil@y.com", "subject": "s", "text": "t"}), "plain email address"),
            (json!({"to": "Dana <dana@x.com>", "subject": "s", "text": "t"}), "plain email address"),
            (json!({"to": "a@x.com,b@y.com", "subject": "s", "text": "t"}), "plain email address"),
            (json!({"to": "dana@x.com", "subject": "s\r\nBcc: evil@y.com", "text": "t"}), "one line"),
            (json!({"to": "dana@x.com", "subject": "s", "text": "  "}), "no text"),
            (json!({"to": "dana@x.com", "subject": "s", "text": "t", "in_reply_to": "obj:message/nope"}), "no stored message"),
        ] {
            let e = send(&mem, &cfg, &args).unwrap_err();
            assert!(e.contains(want), "{args}: {e}");
        }
        assert!(mem.objects_of("sent_message", 10).is_empty());
    }
}
