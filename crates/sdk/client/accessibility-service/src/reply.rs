//! The platform half's JSON replies, parsed. Pure, so it is tested on the
//! host; only the transport is platform code.
//!
//! A reply is `{"ok": value}`, `{"pending": true}` (the result arrives later
//! under the call's token) or `{"error": code, "message": text}`.

use serde_json::Value;

use crate::{policy, Error, Result, Rotation};

/// A parsed reply.
#[derive(Debug, PartialEq)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) enum Reply {
    Ok(Value),
    Pending,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn parse(json: &str) -> Result<Reply> {
    let v: Value = serde_json::from_str(json).map_err(|e| {
        Error::Platform(format!("malformed accessibility-service reply {json:?}: {e}"))
    })?;
    if let Some(code) = v.get("error").and_then(Value::as_str) {
        let message = v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Err(match code {
            "not_enabled" => Error::NotEnabled,
            // The platform refused on its own clock (another caller in the
            // process, or a request the SDK's gate let through early): a
            // full interval is the safe wait.
            "too_soon" => Error::TooSoon {
                retry_after_ms: policy::SCREENSHOT_INTERVAL_MS,
            },
            "secure_window" => Error::SecureWindow,
            "gesture_cancelled" => Error::GestureCancelled,
            "refused" => Error::Refused(message),
            "no_focused_input" => Error::NoFocusedInput,
            "stale_node" => Error::StaleNode,
            "timeout" => Error::Timeout,
            "invalid_argument" => Error::InvalidArgument(message),
            "unsupported" => Error::Unsupported(message),
            _ => Error::Platform(message),
        });
    }
    if v.get("pending").and_then(Value::as_bool) == Some(true) {
        return Ok(Reply::Pending);
    }
    match v.get("ok") {
        Some(value) => Ok(Reply::Ok(value.clone())),
        None => Err(Error::Platform(format!(
            "malformed accessibility-service reply {json:?}"
        ))),
    }
}

/// `Display.getRotation()`: `Surface.ROTATION_0` … `ROTATION_270`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn rotation(surface_rotation: i32) -> Result<Rotation> {
    Ok(match surface_rotation {
        0 => Rotation::Deg0,
        1 => Rotation::Deg90,
        2 => Rotation::Deg180,
        3 => Rotation::Deg270,
        other => return Err(Error::Platform(format!("unknown display rotation {other}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_and_pending_replies_parse() {
        assert_eq!(parse(r#"{"ok":true}"#), Ok(Reply::Ok(Value::Bool(true))));
        assert_eq!(parse(r#"{"ok":null}"#), Ok(Reply::Ok(Value::Null)));
        assert_eq!(parse(r#"{"pending":true}"#), Ok(Reply::Pending));
    }

    #[test]
    fn every_error_code_maps_to_its_variant() {
        let err = |code: &str| parse(&format!(r#"{{"error":"{code}","message":"m"}}"#)).unwrap_err();
        assert_eq!(err("not_enabled"), Error::NotEnabled);
        assert_eq!(err("too_soon"), Error::TooSoon { retry_after_ms: 333 });
        assert_eq!(err("secure_window"), Error::SecureWindow);
        assert_eq!(err("gesture_cancelled"), Error::GestureCancelled);
        assert_eq!(err("refused"), Error::Refused("m".into()));
        assert_eq!(err("no_focused_input"), Error::NoFocusedInput);
        assert_eq!(err("stale_node"), Error::StaleNode);
        assert_eq!(err("timeout"), Error::Timeout);
        assert_eq!(err("invalid_argument"), Error::InvalidArgument("m".into()));
        assert_eq!(err("unsupported"), Error::Unsupported("m".into()));
        assert_eq!(err("platform"), Error::Platform("m".into()));
        assert_eq!(err("from_the_future"), Error::Platform("m".into()));
    }

    #[test]
    fn malformed_replies_are_platform_errors() {
        assert!(matches!(parse("{"), Err(Error::Platform(_))));
        assert!(matches!(parse(r#"{"neither":1}"#), Err(Error::Platform(_))));
    }

    #[test]
    fn surface_rotations_map_to_degrees() {
        assert_eq!(rotation(0), Ok(Rotation::Deg0));
        assert_eq!(rotation(1), Ok(Rotation::Deg90));
        assert_eq!(rotation(2), Ok(Rotation::Deg180));
        assert_eq!(rotation(3), Ok(Rotation::Deg270));
        assert!(rotation(4).is_err());
    }
}
