//! The fingerprint of a bug, ported from support-agent's
//! `engines/fingerprint.ts`: `sha256(exceptionClass | normalised message |
//! first 5 app frames)`, the app-frame prefix a rule option instead of
//! `com.papayapos.`, and the original's similarity score of a candidate
//! bug's summary (exception 50, component 30, message head 20).

use sha2::{Digest, Sha256};

/// Frames of the stack trace that count.
pub const MAX_APP_FRAMES: usize = 5;

/// PURE: the frames of `stack` that are the application's: lines with
/// `prefix` (every `at …` line when it is empty), trimmed, at most five.
pub fn app_frames(stack: &str, prefix: &str) -> Vec<String> {
    stack
        .lines()
        .map(str::trim)
        .filter(|l| {
            if prefix.is_empty() {
                l.starts_with("at ")
            } else {
                l.contains(prefix)
            }
        })
        .take(MAX_APP_FRAMES)
        .map(str::to_string)
        .collect()
}

fn is_uuid_at(b: &[u8], i: usize) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut j = i;
    for (n, g) in GROUPS.iter().enumerate() {
        if j + g > b.len() || !b[j..j + g].iter().all(u8::is_ascii_hexdigit) {
            return false;
        }
        j += g;
        if n < 4 {
            if b.get(j) != Some(&b'-') {
                return false;
            }
            j += 1;
        }
    }
    true
}

/// PURE: a message with its UUIDs as `UUID` and each run of digits as `N`,
/// trimmed and lower-cased.
pub fn normalize_message(message: &str) -> String {
    let b = message.as_bytes();
    let mut out = String::with_capacity(message.len());
    let mut i = 0;
    while i < b.len() {
        if is_uuid_at(b, i) {
            out.push_str("UUID");
            i += 36;
        } else if b[i].is_ascii_digit() {
            out.push('N');
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            let ch = message[i..].chars().next().unwrap_or(' ');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out.trim().to_lowercase()
}

/// PURE: the fingerprint, hex; `None` without an exception class.
pub fn fingerprint(
    exception_class: &str,
    message: &str,
    stack: &str,
    prefix: &str,
) -> Option<String> {
    let class = exception_class.trim();
    if class.is_empty() {
        return None;
    }
    let mut parts = vec![class.to_string(), normalize_message(message)];
    parts.extend(app_frames(stack, prefix));
    Some(hex::encode(Sha256::digest(parts.join("|").as_bytes())))
}

/// PURE: how much a bug's summary looks like this one, 0 to 100.
pub fn similarity(summary: &str, exception_class: &str, component: &str, message: &str) -> u32 {
    let summary = summary.to_lowercase();
    let mut score = 0;
    let class = exception_class.trim().to_lowercase();
    if !class.is_empty() && summary.contains(&class) {
        score += 50;
    }
    let component = component.trim().to_lowercase();
    if !component.is_empty() && component != "unknown" && summary.contains(&component) {
        score += 30;
    }
    let head: String = message
        .trim()
        .chars()
        .take(30)
        .collect::<String>()
        .to_lowercase();
    if !head.is_empty() && summary.contains(&head) {
        score += 20;
    }
    score.min(100)
}

/// A candidate at or above this is the same bug: link to it.
pub const REUSE_AT: u32 = 80;
/// A candidate at or above this is related: create, naming it.
pub const FOLLOW_UP_AT: u32 = 60;

#[cfg(test)]
mod tests {
    use super::*;

    const STACK: &str = "java.lang.NullPointerException: x\n\tat com.papayapos.pay.Gateway.charge(Gateway.java:42)\n\tat org.spring.Proxy.invoke(Proxy.java:1)\n\tat com.papayapos.pay.Checkout.run(Checkout.java:7)\n";

    #[test]
    fn the_message_loses_its_numbers_and_ids() {
        assert_eq!(
            normalize_message(" Order 12345 for 3f2504e0-4f89-11d3-9a0c-0305e82c3301 FAILED "),
            "order n for uuid failed"
        );
        assert_eq!(normalize_message("žiadna platba 7x"), "žiadna platba nx");
    }

    #[test]
    fn app_frames_follow_the_prefix() {
        assert_eq!(
            app_frames(STACK, "com.papayapos."),
            [
                "at com.papayapos.pay.Gateway.charge(Gateway.java:42)",
                "at com.papayapos.pay.Checkout.run(Checkout.java:7)"
            ]
        );
        assert_eq!(app_frames(STACK, "").len(), 3);
    }

    #[test]
    fn a_fingerprint_ignores_ids_and_needs_a_class() {
        let a = fingerprint("NPE", "order 1 failed", STACK, "com.papayapos.").unwrap();
        let b = fingerprint("NPE", "order 2 failed", STACK, "com.papayapos.").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert_ne!(
            a,
            fingerprint("IOException", "order 1 failed", STACK, "com.papayapos.").unwrap()
        );
        assert_eq!(fingerprint(" ", "m", STACK, ""), None);
    }

    #[test]
    fn similarity_scores_as_the_original() {
        let s = "[payments] NullPointerException: Gateway key is null";
        assert_eq!(
            similarity(s, "NullPointerException", "payments", "Gateway key is null"),
            100
        );
        assert_eq!(
            similarity(s, "NullPointerException", "printing", "other"),
            50
        );
        assert_eq!(similarity(s, "", "unknown", ""), 0);
    }
}
