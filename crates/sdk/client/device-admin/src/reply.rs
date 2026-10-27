//! The platform half's JSON replies, parsed. Pure, so it is tested on the
//! host; only the transport is platform code.
//!
//! A reply is `{"ok": value}`, `{"pending": true}` (the result arrives later
//! under the call's token) or `{"error": code, "message": text}`.

use serde_json::Value;

use crate::{Error, InstallFailure, InstallResult, PackageOperation, Result};

/// A parsed reply.
#[derive(Debug, PartialEq)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) enum Reply {
    Ok(Value),
    Pending,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn parse(json: &str) -> Result<Reply> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| Error::Platform(format!("malformed device-admin reply {json:?}: {e}")))?;
    if let Some(code) = v.get("error").and_then(Value::as_str) {
        let message = v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Err(match code {
            "not_device_owner" => Error::NotDeviceOwner,
            "unsupported" => Error::Unsupported(message),
            "certificate_mismatch" => certificate_mismatch(&message),
            _ => Error::Platform(message),
        });
    }
    if v.get("pending").and_then(Value::as_bool) == Some(true) {
        return Ok(Reply::Pending);
    }
    match v.get("ok") {
        Some(value) => Ok(Reply::Ok(value.clone())),
        None => Err(Error::Platform(format!("malformed device-admin reply {json:?}"))),
    }
}

/// `expected=<hex> actual=<hex>`.
fn certificate_mismatch(message: &str) -> Error {
    let field = |name: &str| {
        message
            .split_whitespace()
            .find_map(|part| part.strip_prefix(name))
            .unwrap_or_default()
            .to_string()
    };
    Error::CertificateMismatch {
        expected: field("expected="),
        actual: field("actual="),
    }
}

/// An `install_result` object (the `ok` of an install/uninstall, or the
/// event's body).
pub(crate) fn install_result(v: Value) -> Result<InstallResult> {
    let operation = match v.get("operation").and_then(Value::as_str) {
        Some("uninstall") => PackageOperation::Uninstall,
        _ => PackageOperation::Install,
    };
    let package = v.get("package").and_then(Value::as_str).map(str::to_string);
    let status = v
        .get("status")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::Platform(format!("install result without a status: {v}")))?;
    if status == 0 {
        return Ok(InstallResult::Success {
            operation,
            package,
            version_code: v.get("version_code").and_then(Value::as_i64),
        });
    }
    // `PackageInstaller.STATUS_*`.
    let reason = match status {
        -1 => InstallFailure::PendingUserAction,
        1 => InstallFailure::Generic,
        2 => InstallFailure::Blocked,
        3 => InstallFailure::Aborted,
        4 => InstallFailure::Invalid,
        5 => InstallFailure::Conflict,
        6 => InstallFailure::Storage,
        7 => InstallFailure::Incompatible,
        8 => InstallFailure::Timeout,
        other => InstallFailure::Other(other as i32),
    };
    Ok(InstallResult::Failure {
        operation,
        package,
        reason,
        message: v.get("message").and_then(Value::as_str).map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ok_pending_and_error_replies() {
        assert_eq!(parse(r#"{"ok":true}"#).unwrap(), Reply::Ok(json!(true)));
        assert_eq!(parse(r#"{"ok":null}"#).unwrap(), Reply::Ok(Value::Null));
        assert_eq!(parse(r#"{"pending":true}"#).unwrap(), Reply::Pending);
        assert_eq!(
            parse(r#"{"error":"not_device_owner","message":"x is not the device owner"}"#),
            Err(Error::NotDeviceOwner)
        );
        assert_eq!(
            parse(r#"{"error":"unsupported","message":"setLockTaskFeatures needs API 28"}"#),
            Err(Error::Unsupported("setLockTaskFeatures needs API 28".into()))
        );
        assert_eq!(
            parse(r#"{"error":"platform","message":"SecurityException: no"}"#),
            Err(Error::Platform("SecurityException: no".into()))
        );
        assert!(matches!(parse("{}"), Err(Error::Platform(_))));
        assert!(matches!(parse("not json"), Err(Error::Platform(_))));
    }

    #[test]
    fn a_certificate_mismatch_carries_both_digests() {
        assert_eq!(
            parse(r#"{"error":"certificate_mismatch","message":"expected=ab12 actual=cd34"}"#),
            Err(Error::CertificateMismatch {
                expected: "ab12".into(),
                actual: "cd34".into()
            })
        );
    }

    #[test]
    fn install_results_map_installer_status_codes() {
        assert_eq!(
            install_result(json!({
                "type": "install_result", "operation": "install",
                "package": "com.example.kiosk", "status": 0, "version_code": 1_002_003,
            }))
            .unwrap(),
            InstallResult::Success {
                operation: PackageOperation::Install,
                package: Some("com.example.kiosk".into()),
                version_code: Some(1_002_003),
            }
        );
        let conflict = install_result(json!({
            "operation": "install", "package": "com.example.kiosk", "status": 5,
            "message": "INSTALL_FAILED_VERSION_DOWNGRADE",
        }))
        .unwrap();
        assert_eq!(
            conflict,
            InstallResult::Failure {
                operation: PackageOperation::Install,
                package: Some("com.example.kiosk".into()),
                reason: InstallFailure::Conflict,
                message: Some("INSTALL_FAILED_VERSION_DOWNGRADE".into()),
            }
        );
        assert!(!conflict.is_success());
        let uninstall = install_result(json!({ "operation": "uninstall", "status": -1 })).unwrap();
        assert!(matches!(
            uninstall,
            InstallResult::Failure {
                operation: PackageOperation::Uninstall,
                reason: InstallFailure::PendingUserAction,
                ..
            }
        ));
        assert!(install_result(json!({ "operation": "install" })).is_err());
    }
}
