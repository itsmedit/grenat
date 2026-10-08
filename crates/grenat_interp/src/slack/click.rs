//! A click on an approval's button, as Slack's interactivity sends it: a
//! POST of `application/x-www-form-urlencoded` whose `payload` field is
//! JSON — for a button, a `block_actions` payload: the `user` who clicked,
//! the `actions` (`action_id`, `value`), the `channel` and the `container`
//! (`message_ts`) of the message. Only the fields used are read, each
//! checked: the user and the channel are Slack IDs, the message's time a
//! Slack timestamp, the value an approval's id.

use serde_json::Value as Json;

use super::blocks::{APPROVE, DENY};
use super::is_id;

/// A click on Approve or Deny.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Click {
    pub approval: i64,
    pub approve: bool,
    /// Who clicked (`U0123ABC`).
    pub user: String,
    /// The message clicked: its channel's ID and its timestamp.
    pub channel: String,
    pub message_ts: String,
}

/// The click a request's body carries: `Ok(None)` for another interaction
/// (another app's button, a shortcut), `Err` for a malformed payload.
pub(crate) fn parse(form: &[(String, String)]) -> Result<Option<Click>, String> {
    let Some((_, payload)) = form.iter().find(|(k, _)| k == "payload") else {
        return Err("no `payload` field".into());
    };
    let payload: Json = serde_json::from_str(payload).map_err(|_| "the payload is not JSON".to_string())?;
    if payload["type"] != "block_actions" {
        return Ok(None);
    }
    let Some(action) = payload["actions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| matches!(a["action_id"].as_str(), Some(APPROVE | DENY)))
    else {
        return Ok(None);
    };
    let approve = action["action_id"] == APPROVE;
    let approval = action["value"]
        .as_str()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .ok_or("the button carries no approval")?;
    let id = |value: &Json, what: &str| {
        value.as_str().filter(|v| is_id(v)).map(str::to_string).ok_or_else(|| format!("no {what}"))
    };
    let user = id(&payload["user"]["id"], "user")?;
    let channel =
        id(&payload["channel"]["id"], "channel").or_else(|_| id(&payload["container"]["channel_id"], "channel"))?;
    let message_ts = payload["container"]["message_ts"]
        .as_str()
        .or_else(|| payload["message"]["ts"].as_str())
        .filter(|ts| is_timestamp(ts))
        .map(str::to_string)
        .ok_or("no message")?;
    Ok(Some(Click { approval, approve, user, channel, message_ts }))
}

/// `1548261231.000200`.
fn is_timestamp(ts: &str) -> bool {
    ts.len() <= 32
        && ts.split_once('.').is_some_and(|(s, f)| {
            !s.is_empty() && !f.is_empty() && s.bytes().chain(f.bytes()).all(|b| b.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A `block_actions` payload shaped as Slack's documented example.
    fn payload(action: &str, value: &str, user: &str) -> Json {
        json!({
            "type": "block_actions",
            "team": {"id": "T9TK3CUKW", "domain": "example"},
            "user": {"id": user, "username": "jtorrance", "team_id": "T9TK3CUKW"},
            "api_app_id": "AABA1ABCD",
            "container": {"type": "message", "message_ts": "1548261231.000200", "channel_id": "CBR2V3XEX"},
            "trigger_id": "12321423423.333649436676.d8c1bb837935619ccad0f624c448ffb3",
            "channel": {"id": "CBR2V3XEX", "name": "review-updates"},
            "message": {"bot_id": "BAH5CA16Z", "ts": "1548261231.000200"},
            "response_url": "https://hooks.slack.com/actions/AABA1ABCD/1232321423432/D09sSasdasdAS9091209",
            "actions": [{"action_id": action, "block_id": "grenat_approval", "value": value, "type": "button", "action_ts": "1548426417.840180"}],
        })
    }

    fn form(payload: &Json) -> Vec<(String, String)> {
        vec![("payload".into(), payload.to_string())]
    }

    #[test]
    fn a_click_on_approve_or_deny() {
        let click = parse(&form(&payload(APPROVE, "5", "UA8RXUSPL"))).unwrap().unwrap();
        assert_eq!(
            click,
            Click {
                approval: 5,
                approve: true,
                user: "UA8RXUSPL".into(),
                channel: "CBR2V3XEX".into(),
                message_ts: "1548261231.000200".into()
            }
        );
        assert!(!parse(&form(&payload(DENY, "5", "UA8RXUSPL"))).unwrap().unwrap().approve);
    }

    #[test]
    fn other_interactions_are_not_clicks() {
        assert_eq!(parse(&form(&payload("another_app", "5", "U1"))), Ok(None));
        assert_eq!(parse(&form(&json!({"type": "shortcut"}))), Ok(None));
    }

    #[test]
    fn a_malformed_payload_is_refused() {
        assert!(parse(&[]).is_err());
        assert!(parse(&[("payload".into(), "{".into())]).is_err());
        assert!(parse(&form(&payload(APPROVE, "five", "U1"))).is_err());
        assert!(parse(&form(&payload(APPROVE, "-1", "U1"))).is_err());
        assert!(parse(&form(&payload(APPROVE, "5", "<!channel>"))).is_err());
        let mut no_ts = payload(APPROVE, "5", "U1");
        no_ts["container"]["message_ts"] = json!("now");
        no_ts["message"]["ts"] = json!(null);
        assert!(parse(&form(&no_ts)).is_err());
    }
}
