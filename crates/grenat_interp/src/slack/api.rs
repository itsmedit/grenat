//! Slack's Web API: a method (`chat.postMessage`, `chat.update`,
//! `chat.postEphemeral`) is a POST of JSON to `https://slack.com/api/<method>`
//! with the bot token as a bearer token. Slack answers HTTP 200 with
//! `{"ok": true, …}`, or `{"ok": false, "error": "channel_not_found"}`; 429
//! when the method is rate limited. An error never holds the token.

use std::time::Duration;

use serde_json::Value as Json;

use crate::http::{HttpReply, HttpRequest, ProxyChoice};

/// The request calling `method` with `body`.
pub(crate) fn request(base: &str, method: &str, token: &str, body: &Json, timeout: Duration) -> HttpRequest {
    HttpRequest {
        method: "POST",
        url: format!("{}/{method}", base.trim_end_matches('/')),
        headers: vec![
            ("Authorization".into(), format!("Bearer {token}")),
            ("Content-Type".into(), "application/json; charset=utf-8".into()),
        ],
        body: Some(body.to_string()),
        timeout,
        proxy: ProxyChoice::Environment,
    }
}

/// What `method` answered: its fields when `ok`, else why not.
pub(crate) fn reply(method: &str, reply: &HttpReply) -> Result<Json, String> {
    if reply.status == 429 {
        let after = reply.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("retry-after")).map(|(_, v)| v.as_str());
        let after = after.filter(|v| v.bytes().all(|b| b.is_ascii_digit())).unwrap_or("?");
        return Err(format!("{method}: rate limited (retry after {after} s)"));
    }
    if !(200..300).contains(&reply.status) {
        return Err(format!("{method}: HTTP {}", reply.status));
    }
    let json: Json = serde_json::from_str(&reply.body).map_err(|_| format!("{method}: the answer is not JSON"))?;
    if json["ok"] == true {
        return Ok(json);
    }
    // Slack's error codes are short words: anything else is not repeated
    let code =
        json["error"].as_str().filter(|e| e.len() <= 64 && e.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    Err(format!("{method}: {}", code.unwrap_or("refused")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn answer(status: u16, body: &str) -> HttpReply {
        HttpReply { status, headers: vec![("Retry-After".into(), "30".into())], body: body.into(), answered_by: None }
    }

    #[test]
    fn a_method_is_a_post_of_json_with_the_token() {
        let body = json!({"channel": "#ops", "text": "hi"});
        let r = request("https://slack.com/api/", "chat.postMessage", "xoxb-1", &body, Duration::from_secs(5));
        assert_eq!((r.method, r.url.as_str()), ("POST", "https://slack.com/api/chat.postMessage"));
        assert_eq!(r.headers[0], ("Authorization".into(), "Bearer xoxb-1".into()));
        assert_eq!(r.body.as_deref(), Some(r##"{"channel":"#ops","text":"hi"}"##));
    }

    #[test]
    fn answers() {
        let ok = reply("chat.postMessage", &answer(200, r#"{"ok":true,"ts":"1.2"}"#)).unwrap();
        assert_eq!(ok["ts"], "1.2");
        let e = |status, body| reply("chat.update", &answer(status, body)).unwrap_err();
        assert_eq!(e(200, r#"{"ok":false,"error":"channel_not_found"}"#), "chat.update: channel_not_found");
        assert_eq!(e(200, r#"{"ok":false,"error":"<b>xoxb-1</b>"}"#), "chat.update: refused");
        assert_eq!(e(200, "<html>"), "chat.update: the answer is not JSON");
        assert_eq!(e(429, ""), "chat.update: rate limited (retry after 30 s)");
        assert_eq!(e(500, ""), "chat.update: HTTP 500");
    }
}
