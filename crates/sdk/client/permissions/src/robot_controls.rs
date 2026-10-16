//! Robot test controls: fake the OS's permission answers, so an E2E test
//! can assume a grant (or a denial) instead of meeting a system prompt it
//! can't tap. Compiled only into robot builds (`runtime_vocabulary::
//! robot_only!` in `lib.rs`).
//!
//! While a permission is faked, [`crate::status`] reports the fake and
//! [`crate::request`] resolves to it without prompting. Every `request` is
//! logged, faked or not, so a test can assert the app asked.

use std::sync::Mutex;

use runtime_core::robot_fn;
use runtime_vocabulary::robot_fn::register as register_fn;

use crate::{Permission, PermissionStatus};

/// Faked answers. A `Mutex`: a request's future may resolve on whichever
/// thread the platform answers it.
static FAKES: Mutex<Vec<(Permission, PermissionStatus)>> = Mutex::new(Vec::new());
static REQUESTS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

pub(crate) fn fake(permission: Permission) -> Option<PermissionStatus> {
    FAKES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(p, _)| *p == permission).map(|(_, s)| *s)
}

pub(crate) fn record_request(permission: Permission) {
    REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).push(name_of(permission));
}

const NAMES: [(&str, Permission); 5] = [
    ("notifications", Permission::Notifications),
    ("location_when_in_use", Permission::LocationWhenInUse),
    ("location_always", Permission::LocationAlways),
    ("camera", Permission::Camera),
    ("microphone", Permission::Microphone),
];

fn name_of(permission: Permission) -> &'static str {
    NAMES.iter().find(|(_, p)| *p == permission).map(|(n, _)| *n).unwrap_or("unknown")
}

fn parse_permission(name: &str) -> Result<Permission, String> {
    NAMES.iter().find(|(n, _)| *n == name).map(|(_, p)| *p).ok_or_else(|| {
        format!("unknown permission '{name}' ({})", NAMES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join("|"))
    })
}

fn parse_status(name: &str) -> Result<PermissionStatus, String> {
    Ok(match name {
        "granted" => PermissionStatus::Granted,
        "denied" => PermissionStatus::Denied,
        "restricted" => PermissionStatus::Restricted,
        "undetermined" => PermissionStatus::Undetermined,
        "unsupported" => PermissionStatus::Unsupported,
        other => return Err(format!("unknown status '{other}' (granted|denied|restricted|undetermined|unsupported)")),
    })
}

/// Fake the OS's answer for `permission` (`camera`, `microphone`,
/// `notifications`, `location_when_in_use`, `location_always`): `status()`
/// reports `status` (`granted`, `denied`, `restricted`, `undetermined`,
/// `unsupported`) and `request()` resolves to it without a prompt.
#[robot_fn(name = "permissions.set")]
fn set(permission: String, status: String) -> Result<(), String> {
    let (permission, status) = (parse_permission(&permission)?, parse_status(&status)?);
    let mut fakes = FAKES.lock().unwrap_or_else(|e| e.into_inner());
    fakes.retain(|(p, _)| *p != permission);
    fakes.push((permission, status));
    Ok(())
}

/// Stop faking `permission` (every permission when left out): the OS
/// answers again.
#[robot_fn(name = "permissions.clear")]
fn clear(permission: Option<String>) -> Result<(), String> {
    let only = permission.as_deref().map(parse_permission).transpose()?;
    FAKES.lock().unwrap_or_else(|e| e.into_inner()).retain(|(p, _)| only.is_some_and(|o| o != *p));
    Ok(())
}

/// Every permission the app requested since launch (or the last read with
/// `clear: true`), oldest first, faked or not.
#[robot_fn(name = "permissions.requests")]
fn requests(clear: Option<bool>) -> Vec<&'static str> {
    let mut log = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
    let out = log.clone();
    if clear.unwrap_or(false) {
        log.clear();
    }
    out
}

pub(crate) fn register() {
    register_fn(set::robot());
    register_fn(clear::robot());
    register_fn(requests::robot());
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::__serde_json::{json, Value};
    use runtime_vocabulary::robot::bridge::invoke_command;

    fn call(name: &str, args: Value) -> Result<Value, String> {
        invoke_command("call_function", &json!({ "name": name, "args": args }))
            .map(|s| runtime_core::__serde_json::from_str::<Value>(&s).unwrap()["value"].clone())
    }

    fn now<F: std::future::Future>(f: F) -> F::Output {
        let mut f = std::pin::pin!(f);
        match f.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
            std::task::Poll::Ready(v) => v,
            std::task::Poll::Pending => panic!("a faked permission answers without waiting"),
        }
    }

    /// One test: the fakes and the request log are process-wide.
    #[test]
    fn a_faked_permission_answers_status_and_request_without_a_prompt() {
        crate::register_robot_controls();
        call("permissions.requests", json!({ "clear": true })).unwrap();

        call("permissions.set", json!({ "permission": "camera", "status": "granted" })).unwrap();
        assert_eq!(now(crate::status(Permission::Camera)), PermissionStatus::Granted);
        assert_eq!(now(crate::request(Permission::Camera)), PermissionStatus::Granted);
        call("permissions.set", json!({ "permission": "camera", "status": "denied" })).unwrap();
        assert_eq!(now(crate::request(Permission::Camera)), PermissionStatus::Denied, "re-set replaces");
        assert_eq!(call("permissions.requests", json!({})).unwrap(), json!(["camera", "camera"]));

        let err = call("permissions.set", json!({ "permission": "telepathy", "status": "granted" })).unwrap_err();
        assert!(err.contains("unknown permission"), "{err}");
        let err = call("permissions.set", json!({ "permission": "camera", "status": "maybe" })).unwrap_err();
        assert!(err.contains("unknown status"), "{err}");

        call("permissions.set", json!({ "permission": "microphone", "status": "granted" })).unwrap();
        call("permissions.clear", json!({ "permission": "camera" })).unwrap();
        assert_eq!(fake(Permission::Camera), None);
        assert_eq!(fake(Permission::Microphone), Some(PermissionStatus::Granted));
        call("permissions.clear", json!({})).unwrap();
        assert_eq!(fake(Permission::Microphone), None);
    }
}
