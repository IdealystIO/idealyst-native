//! Package-level policy replies (listing, the packages that must never be
//! hidden), parsed. Pure, so it is tested on the host.

use serde_json::Value;

use crate::{Error, PackageInfo, PackageVersion, ProtectedReason, Result};

/// The packages hiding or suspending would strand the device with: the app
/// itself, the lock-task allow-list and the current home screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Protected {
    pub(crate) own: String,
    pub(crate) lock_task: Vec<String>,
    pub(crate) home: Option<String>,
}

impl Protected {
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn parse(v: &Value) -> Result<Self> {
        let own = v
            .get("own")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("protected packages", v))?
            .to_string();
        Ok(Self {
            own,
            lock_task: strings(v.get("lock_task").unwrap_or(&Value::Null))?,
            home: v.get("home").and_then(Value::as_str).map(str::to_string),
        })
    }

    /// `Err(ProtectedPackage)` for the first package in `packages` that is
    /// protected.
    pub(crate) fn check(&self, packages: &[&str]) -> Result<()> {
        for &package in packages {
            let reason = if package == self.own {
                ProtectedReason::OwnPackage
            } else if self.lock_task.iter().any(|p| p == package) {
                ProtectedReason::LockTask
            } else if self.home.as_deref() == Some(package) {
                ProtectedReason::Home
            } else {
                continue;
            };
            return Err(Error::ProtectedPackage {
                package: package.to_string(),
                reason,
            });
        }
        Ok(())
    }
}

/// A JSON array of strings.
pub(crate) fn strings(v: &Value) -> Result<Vec<String>> {
    v.as_array()
        .ok_or_else(|| malformed("package list", v))?
        .iter()
        .map(|p| p.as_str().map(str::to_string).ok_or_else(|| malformed("package name", p)))
        .collect()
}

/// `installed_packages`' reply.
pub(crate) fn package_infos(v: &Value) -> Result<Vec<PackageInfo>> {
    v.as_array()
        .ok_or_else(|| malformed("package list", v))?
        .iter()
        .map(package_info)
        .collect()
}

fn package_info(v: &Value) -> Result<PackageInfo> {
    let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let flag = |key: &str| {
        v.get(key)
            .and_then(Value::as_bool)
            .ok_or_else(|| malformed(key, v))
    };
    Ok(PackageInfo {
        package: text("package").ok_or_else(|| malformed("package", v))?,
        label: text("label").unwrap_or_default(),
        version: PackageVersion {
            name: text("version_name"),
            code: v
                .get("version_code")
                .and_then(Value::as_i64)
                .ok_or_else(|| malformed("version_code", v))?,
        },
        is_system: flag("is_system")?,
        is_installed: flag("is_installed")?,
        is_hidden: flag("is_hidden")?,
        is_suspended: flag("is_suspended")?,
        is_enabled: flag("is_enabled")?,
    })
}

fn malformed(what: &str, v: &Value) -> Error {
    Error::Platform(format!("malformed {what} in device-admin reply: {v}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn protected() -> Protected {
        Protected::parse(&json!({
            "own": "com.example.agent",
            "lock_task": ["com.example.kiosk", "com.example.agent"],
            "home": "com.example.kiosk",
        }))
        .unwrap()
    }

    fn refused(packages: &[&str]) -> (String, ProtectedReason) {
        match protected().check(packages) {
            Err(Error::ProtectedPackage { package, reason }) => (package, reason),
            other => panic!("expected ProtectedPackage, got {other:?}"),
        }
    }

    #[test]
    fn hiding_the_app_itself_the_kiosk_or_the_home_screen_is_refused() {
        assert_eq!(refused(&["com.example.agent"]), ("com.example.agent".into(), ProtectedReason::OwnPackage));
        assert_eq!(refused(&["com.example.kiosk"]), ("com.example.kiosk".into(), ProtectedReason::LockTask));
        let p = Protected {
            lock_task: vec![],
            ..protected()
        };
        assert!(matches!(
            p.check(&["com.example.kiosk"]),
            Err(Error::ProtectedPackage { reason: ProtectedReason::Home, .. })
        ));
    }

    #[test]
    fn one_protected_package_in_a_batch_refuses_the_batch() {
        assert_eq!(
            refused(&["com.lenovo.entertainment", "com.example.kiosk"]).0,
            "com.example.kiosk"
        );
    }

    #[test]
    fn bundled_apps_are_not_protected() {
        assert_eq!(protected().check(&["com.lenovo.entertainment", "com.android.chrome"]), Ok(()));
        // No resolvable home and no lock-task list (API < 26).
        let bare = Protected::parse(&json!({ "own": "a", "lock_task": [], "home": null })).unwrap();
        assert_eq!(bare.check(&["b"]), Ok(()));
    }

    #[test]
    fn package_infos_parse_and_keep_hidden_packages() {
        let list = package_infos(&json!([
            {
                "package": "com.lenovo.entertainment", "label": "Entertainment Space",
                "version_name": "1.2", "version_code": 12,
                "is_system": true, "is_installed": true, "is_hidden": true,
                "is_suspended": false, "is_enabled": true
            },
            {
                "package": "com.example.kiosk", "label": "Kiosk",
                "version_name": null, "version_code": 3,
                "is_system": false, "is_installed": true, "is_hidden": false,
                "is_suspended": true, "is_enabled": true
            }
        ]))
        .unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].is_hidden && list[0].is_system);
        assert_eq!(list[0].version, PackageVersion { name: Some("1.2".into()), code: 12 });
        assert_eq!(list[1].version.name, None);
        assert!(list[1].is_suspended);
    }

    #[test]
    fn a_package_missing_a_flag_is_malformed_not_defaulted() {
        let r = package_infos(&json!([{ "package": "p", "version_code": 1 }]));
        assert!(matches!(r, Err(Error::Platform(_))), "{r:?}");
    }

    #[test]
    fn failed_suspensions_parse_as_names() {
        assert_eq!(strings(&json!(["a", "b"])).unwrap(), ["a", "b"]);
        assert!(strings(&json!([1])).is_err());
    }
}
