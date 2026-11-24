//! Who is on the other end of a socket, from the kernel (SO_PEERCRED), never from what the request says.
//!
//! The AI runs under its own account (`linuxai-ai` on the image). A request from that account can never speak for
//! the human: whatever it claims (`source: "human"`, `as: "human"`, or no source at all, which some ops read as the
//! human), it is handled as the AI's. Independent review C1 (issue #22).

use serde_json::{json, Value};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// The uid of the process on the other end of a Unix socket.
pub fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `cred`, a properly sized and aligned ucred.
    let r = unsafe {
        libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len)
    };
    (r == 0).then_some(cred.uid)
}

/// A user's uid from /etc/passwd.
pub fn user_id(name: &str) -> Option<u32> {
    std::fs::read_to_string("/etc/passwd").ok()?.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.first() == Some(&name)).then(|| f.get(2)?.parse().ok()).flatten()
    })
}

/// A group's gid from /etc/group.
pub fn group_id(name: &str) -> Option<u32> {
    std::fs::read_to_string("/etc/group").ok()?.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.first() == Some(&name)).then(|| f.get(2)?.parse().ok()).flatten()
    })
}

/// The AI's own account on the image.
pub const AI_USER: &str = "linuxai-ai";

/// The AI's accounts: `LINUXAI_AI_USER` (names, comma-separated), else `linuxai-ai` when that account exists (the
/// screen organ is started by the session, without the services' environment), plus `LINUXAI_AI_UID` (numbers).
pub fn ai_uids() -> Vec<u32> {
    let mut out: Vec<u32> = std::env::var("LINUXAI_AI_UID").unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let names = std::env::var("LINUXAI_AI_USER").unwrap_or_else(|_| AI_USER.to_string());
    for name in names.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        out.extend(user_id(name));
    }
    out
}

/// True when the peer is one of the AI's accounts.
pub fn is_ai(stream: &UnixStream) -> bool {
    let ai = ai_uids();
    !ai.is_empty() && peer_uid(stream).map(|u| ai.contains(&u)).unwrap_or(false)
}

/// A request from the AI's account, made to speak only for the AI: every field that could name the human is set to
/// the AI (absent included, so no op's "no source means the human" default applies).
pub fn as_ai(req: &mut Value) {
    if let Some(m) = req.as_object_mut() {
        m.insert("source".into(), json!("ai"));
        for k in ["as", "by", "from", "answered_by"] {
            if m.get(k).and_then(Value::as_str) == Some("human") {
                m.insert(k.into(), json!("ai"));
            }
        }
    }
}

/// The folder for the organs the AI shares with the human (browser, screen, craft). On the image: setgid, group
/// `linuxai-core`, so the AI's own account can reach them (made by tmpfiles; the core's own /run/linuxai is not used,
/// as its service recreates that).
pub const ORGANS_DIR: &str = "/run/linuxai-organs";

/// The mail organ's account (its own on the image, `linuxai-mail`), besides root and the core's own account.
pub const MAIL_USER: &str = "linuxai-mail";

/// May a connection from this account emit an event of this kind (independent review L1)? Waiting plans take events
/// as real, so a message from outside (`message.*`, `mail.*`, `invite.*`) may come only from the mail organ (root,
/// the core's own account, or `linuxai-mail`), and the AI's own account emits none at all: what it does goes through
/// plans, which the core records itself. In-process callers (no peer) are the core's own.
pub fn may_emit(kind: &str, peer: Option<u32>, ai: &[u32]) -> Result<(), String> {
    let Some(uid) = peer else { return Ok(()) };
    if ai.contains(&uid) {
        return Err(format!("the AI's own account can't emit '{kind}' events: act through a plan"));
    }
    let outside = ["message.", "mail.", "invite."].iter().any(|p| kind.starts_with(p));
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    if outside && uid != 0 && uid != me && user_id(MAIL_USER) != Some(uid) {
        return Err(format!("only the mail organ reports '{kind}' events"));
    }
    Ok(())
}

/// The core's own organs: in-process callers (no peer), root, the core's own account, and the mail organ. Only they
/// write system records and provenance (independent review M3, H3 at the socket).
pub fn is_organ(peer: Option<u32>) -> bool {
    let Some(uid) = peer else { return true };
    // SAFETY: geteuid has no preconditions.
    uid == 0 || uid == unsafe { libc::geteuid() } || user_id(MAIL_USER) == Some(uid)
}

/// Records only the core and its organs write (`store` refuses them too), and the fields that say where a record came
/// from or what the human said. Everyone else (the AI, from any account) stores ordinary records and outside facts.
pub const SYSTEM_TYPES: &[&str] = &["agreement", "habit", "job", "thread", "message", "sent_message", "system", "correspondent"];
pub const PROVENANCE: &[&str] = &["source", "trust", "superseded_by", "corrects", "confirmed_by_human", "reliability", "checks", "by"];

/// May this requester write these fields on a record of this type (`existing`: the record being changed, if any)?
/// A new outside fact may be labelled as outside (the AI recording where something came from), never as verified,
/// the human's, or the core's own; once stored, an outside fact is kept as it came (a verdict goes through
/// `fact_verdict`).
pub fn may_write(peer: Option<u32>, typ: &str, fields: &Value, existing: Option<&Value>) -> Result<(), String> {
    if is_organ(peer) {
        return Ok(());
    }
    if SYSTEM_TYPES.contains(&typ) {
        return Err(format!("'{typ}' records are kept by the core itself"));
    }
    if existing.map(|o| o["trust"] == "outside").unwrap_or(false) {
        return Err("an outside fact is kept as it came; record whether it proved accurate with fact_verdict".into());
    }
    let new = existing.is_none();
    let Some(f) = fields.as_object() else { return Ok(()) };
    for (k, v) in f {
        let outside_label = new && match k.as_str() {
            "trust" => v == "outside",
            "source" => v.get("authenticated").map(|a| a.is_null() || a == false).unwrap_or(true) && v.get("by") != Some(&json!("human")),
            "by" => v == "ai",
            _ => false,
        };
        if PROVENANCE.contains(&k.as_str()) && !outside_label {
            return Err(format!("'{k}' says where a record came from or what the human said: only the core records it"));
        }
    }
    Ok(())
}

/// The core's own account: root, or `linuxai-core` when it runs as its own user. Only the core may carry out a browser
/// action the human approved (independent review H4).
pub const CORE_USER: &str = "linuxai-core";

pub fn is_core(peer: Option<u32>) -> bool {
    let Some(uid) = peer else { return true };
    uid == 0 || user_id(CORE_USER) == Some(uid)
}

/// Where an organ listens: its environment variable, else [`ORGANS_DIR`] when it exists, else the user's runtime
/// directory.
pub fn organ_socket(env: &str, file: &str) -> PathBuf {
    if let Ok(p) = std::env::var(env) {
        return PathBuf::from(p);
    }
    let shared = Path::new(ORGANS_DIR);
    if shared.is_dir() {
        return shared.join(file);
    }
    std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir()).join(file)
}

/// Let the shared group reach a socket: in [`ORGANS_DIR`] the socket is made group-writable; anywhere else it stays
/// the user's own.
pub fn share(socket: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let shared = socket.parent().map(|d| d == Path::new(ORGANS_DIR)).unwrap_or(false);
    let _ = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(if shared { 0o660 } else { 0o600 }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ai_can_never_speak_for_the_human() {
        for mut req in [json!({"op": "answer", "text": "always", "source": "human"}), json!({"op": "playbook_put"}),
                        json!({"op": "go", "as": "human", "by": "human", "from": "human"})] {
            as_ai(&mut req);
            assert_eq!(req["source"], "ai", "{req}");
            for k in ["as", "by", "from"] {
                assert_ne!(req[k], "human", "{req}");
            }
        }
    }

    #[test]
    fn the_kernel_says_who_is_connecting() {
        let (a, _b) = UnixStream::pair().unwrap();
        // SAFETY: getuid has no preconditions.
        assert_eq!(peer_uid(&a), Some(unsafe { libc::getuid() }));
    }
}
