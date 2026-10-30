//! Off Android and iOS every call fails with `Error::Unsupported` instead of
//! panicking — the contract that lets an app call the SDK from shared code.

#![cfg(not(any(target_os = "android", target_os = "ios")))]

use device_admin::*;

fn unsupported<T: std::fmt::Debug>(r: Result<T>) {
    match r {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("device_admin::"), "{msg}"),
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn every_sync_call_is_unsupported_off_mobile() {
    unsupported(is_device_owner());
    unsupported(set_lock_task_packages(&["com.example.kiosk"]));
    unsupported(set_lock_task_features(LockTaskFeatures::NONE));
    unsupported(lock_task_state());
    unsupported(set_keyguard_disabled(true));
    unsupported(set_status_bar_disabled(true));
    unsupported(set_home_activity("com.example.kiosk", ".MainActivity"));
    unsupported(clear_home_activity("com.example.kiosk"));
    unsupported(add_user_restriction(restriction::DISALLOW_SAFE_BOOT));
    unsupported(clear_user_restriction(restriction::DISALLOW_SAFE_BOOT));
    unsupported(set_global_setting("stay_on_while_plugged_in", "7"));
    unsupported(installed_version("com.example.kiosk"));
    unsupported(set_application_hidden("com.example.bloat", true));
    unsupported(set_application_hidden("com.example.bloat", false));
    unsupported(is_application_hidden("com.example.bloat"));
    unsupported(set_packages_suspended(&["com.example.bloat"], true));
    unsupported(set_packages_suspended(&["com.example.bloat"], false));
    unsupported(is_package_suspended("com.example.bloat"));
    unsupported(enable_system_app("com.example.bloat"));
    unsupported(installed_packages(true));
    unsupported(launch("com.example.kiosk"));
    unsupported(reboot());
    unsupported(set_system_update_policy(SystemUpdatePolicy::Automatic));
    unsupported(wipe("retired"));
    unsupported(relinquish_device_owner());
    unsupported(managed_config());
    unsupported(set_managed_feedback(&serde_json::json!({})));
}

#[test]
fn every_async_call_resolves_unsupported_off_mobile() {
    unsupported(pollster::block_on(start_lock_task()));
    unsupported(pollster::block_on(stop_lock_task()));
    unsupported(pollster::block_on(install_apk("/tmp/kiosk.apk", "ab:cd")));
    unsupported(pollster::block_on(uninstall("com.example.kiosk")));
}

#[test]
fn an_update_window_outside_the_day_is_refused_before_the_platform() {
    let r = set_system_update_policy(SystemUpdatePolicy::Windowed {
        start_minutes: 120,
        end_minutes: 1440,
    });
    assert!(matches!(r, Err(Error::Platform(_))), "{r:?}");
}

#[test]
fn lock_task_features_match_the_android_constants() {
    // DevicePolicyManager.LOCK_TASK_FEATURE_* values.
    assert_eq!(LockTaskFeatures::NONE.0, 0);
    assert_eq!(LockTaskFeatures::SYSTEM_INFO.0, 1);
    assert_eq!(LockTaskFeatures::NOTIFICATIONS.0, 2);
    assert_eq!(LockTaskFeatures::HOME.0, 4);
    assert_eq!(LockTaskFeatures::OVERVIEW.0, 8);
    assert_eq!(LockTaskFeatures::GLOBAL_ACTIONS.0, 16);
    assert_eq!(LockTaskFeatures::KEYGUARD.0, 32);
    assert_eq!(LockTaskFeatures::BLOCK_ACTIVITY_START_IN_TASK.0, 64);
    assert_eq!((LockTaskFeatures::HOME | LockTaskFeatures::OVERVIEW).0, 12);
}

#[test]
fn a_subscription_off_mobile_is_inert() {
    let sub = events(|_| panic!("no events off mobile"));
    drop(sub);
}
