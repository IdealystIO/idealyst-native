//! QR-code provisioning payloads for a device-owner app.
//!
//! On a freshly reset Android device, tapping the setup wizard's welcome
//! screen six times opens a QR scanner. The code it scans is a JSON object
//! naming the device-admin receiver to make device owner, where to download
//! the APK, and the SHA-256 of the APK's signing certificate (the wizard
//! refuses an APK signed by anyone else). An optional "admin extras" bundle
//! is handed to the app when provisioning completes.
//!
//! Everything app-specific is read off the built APK itself — its package
//! name, the receiver guarded by `android.permission.BIND_DEVICE_ADMIN`
//! (whichever SDK declared it), and its signing certificate — so the payload
//! always matches what the URL serves.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

const BIND_DEVICE_ADMIN: &str = "android.permission.BIND_DEVICE_ADMIN";

/// What a provisioning payload needs from an APK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApkProvisioningInfo {
    pub package: String,
    /// Fully-qualified class name of the device-admin receiver.
    pub receiver: String,
    /// SHA-256 of the signing certificate.
    pub cert_sha256: [u8; 32],
}

impl ApkProvisioningInfo {
    /// `package/receiver`, the form `PROVISIONING_DEVICE_ADMIN_COMPONENT_NAME`
    /// and `dpm set-device-owner` take.
    pub fn component(&self) -> String {
        format!("{}/{}", self.package, self.receiver)
    }

    /// The certificate checksum as the provisioning payload spells it:
    /// URL-safe base64 without padding.
    pub fn signature_checksum(&self) -> String {
        base64url_no_pad(&self.cert_sha256)
    }
}

/// Read the package, device-admin receiver and signing certificate out of a
/// signed APK, using the newest installed build-tools' `aapt2` and
/// `apksigner`.
pub fn inspect_apk(apk: &Path) -> Result<ApkProvisioningInfo> {
    let sdk = crate::find_android_sdk()?;
    let build_tools = crate::pick_latest_dir(&sdk.join("build-tools"))
        .context("no build-tools installed (run `sdkmanager 'build-tools;36.0.0'`)")?;
    let dump = Command::new(build_tools.join("aapt2"))
        .args(["dump", "xmltree", "--file", "AndroidManifest.xml"])
        .arg(apk)
        .output()
        .context("spawn aapt2 dump xmltree")?;
    if !dump.status.success() {
        bail!(
            "aapt2 dump xmltree {} failed: {}",
            apk.display(),
            String::from_utf8_lossy(&dump.stderr)
        );
    }
    let (package, receiver) = device_admin_receiver(&String::from_utf8_lossy(&dump.stdout))
        .with_context(|| format!("reading the manifest of {}", apk.display()))?;
    let certs = Command::new(build_tools.join("apksigner"))
        .args(["verify", "--print-certs"])
        .arg(apk)
        .output()
        .context("spawn apksigner verify")?;
    if !certs.status.success() {
        bail!(
            "apksigner verify {} failed (is it signed?): {}",
            apk.display(),
            String::from_utf8_lossy(&certs.stderr)
        );
    }
    let cert_sha256 = signer_cert_sha256(&String::from_utf8_lossy(&certs.stdout))
        .with_context(|| format!("reading the signing certificate of {}", apk.display()))?;
    Ok(ApkProvisioningInfo {
        package,
        receiver,
        cert_sha256,
    })
}

/// The provisioning payload for `info`, downloaded from `download_url`
/// (https only — the setup wizard refuses anything else), with `extras`
/// (a JSON object) as the admin extras bundle. Provisioning removes the
/// bundled system apps; [`payload_with`] can keep them.
pub fn payload(info: &ApkProvisioningInfo, download_url: &str, extras: Option<&Value>) -> Result<Value> {
    payload_with(info, download_url, extras, false)
}

/// [`payload`], where `leave_system_apps` keeps the
/// manufacturer's bundled apps installed; by default provisioning removes
/// the system apps that have a launcher icon, apart from those the device
/// marks as required, which is what a kiosk wants
/// (`device_admin::enable_system_app` restores one later).
pub fn payload_with(
    info: &ApkProvisioningInfo,
    download_url: &str,
    extras: Option<&Value>,
    leave_system_apps: bool,
) -> Result<Value> {
    if !download_url.starts_with("https://") {
        bail!("--download-url must be an https:// URL (got {download_url:?})");
    }
    let mut out = Map::new();
    out.insert(
        "android.app.extra.PROVISIONING_DEVICE_ADMIN_COMPONENT_NAME".into(),
        json!(info.component()),
    );
    out.insert(
        "android.app.extra.PROVISIONING_DEVICE_ADMIN_PACKAGE_DOWNLOAD_LOCATION".into(),
        json!(download_url),
    );
    out.insert(
        "android.app.extra.PROVISIONING_DEVICE_ADMIN_SIGNATURE_CHECKSUM".into(),
        json!(info.signature_checksum()),
    );
    if leave_system_apps {
        out.insert(
            "android.app.extra.PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED".into(),
            json!(true),
        );
    }
    if let Some(extras) = extras {
        let Some(obj) = extras.as_object() else {
            bail!("--extras must be a JSON object");
        };
        // A PersistableBundle holds strings, numbers, booleans and nested
        // bundles; arrays of mixed type and null do not survive the trip.
        check_bundle_values(obj)?;
        out.insert(
            "android.app.extra.PROVISIONING_ADMIN_EXTRAS_BUNDLE".into(),
            Value::Object(obj.clone()),
        );
    }
    Ok(Value::Object(out))
}

fn check_bundle_values(obj: &Map<String, Value>) -> Result<()> {
    for (k, v) in obj {
        match v {
            Value::String(_) | Value::Number(_) | Value::Bool(_) => {}
            Value::Object(inner) => check_bundle_values(inner)?,
            Value::Null | Value::Array(_) => {
                bail!("--extras key {k:?}: values must be strings, numbers, booleans or objects")
            }
        }
    }
    Ok(())
}

/// Find the package name and the `BIND_DEVICE_ADMIN` receiver in
/// `aapt2 dump xmltree --file AndroidManifest.xml` output.
///
/// The dump prints one element per `E:` line followed by its attributes on
/// `A:` lines, e.g.
/// `A: http://schemas.android.com/apk/res/android:name(0x01010003)="io.x.Admin" (Raw: "io.x.Admin")`.
fn device_admin_receiver(dump: &str) -> Result<(String, String)> {
    struct Element {
        tag: String,
        attrs: Vec<(String, String)>,
    }
    let mut elements: Vec<Element> = Vec::new();
    for line in dump.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("E: ") {
            let tag = rest.split_whitespace().next().unwrap_or_default().to_string();
            elements.push(Element { tag, attrs: Vec::new() });
        } else if let Some(rest) = line.strip_prefix("A: ") {
            let Some(el) = elements.last_mut() else { continue };
            let Some((name, value)) = parse_attr(rest) else { continue };
            el.attrs.push((name, value));
        }
    }
    let attr = |el: &Element, name: &str| {
        el.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    };
    let package = elements
        .iter()
        .find(|e| e.tag == "manifest")
        .and_then(|e| attr(e, "package"))
        .ok_or_else(|| anyhow::anyhow!("the manifest declares no package"))?;
    let receivers: Vec<String> = elements
        .iter()
        .filter(|e| e.tag == "receiver" && attr(e, "permission").as_deref() == Some(BIND_DEVICE_ADMIN))
        .filter_map(|e| attr(e, "name"))
        .collect();
    let receiver = match receivers.as_slice() {
        [] => bail!(
            "no <receiver> guarded by {BIND_DEVICE_ADMIN}; the app needs a device-admin \
             receiver (the `device-admin` SDK declares one)"
        ),
        [one] => one.clone(),
        many => bail!("several device-admin receivers ({}); expected one", many.join(", ")),
    };
    // A relative class name (`.Admin`) is relative to the package.
    let receiver = if receiver.starts_with('.') {
        format!("{package}{receiver}")
    } else {
        receiver
    };
    Ok((package, receiver))
}

/// `<ns>:name(0x…)="value" (Raw: …)` → `("name", "value")`. Attributes
/// without a namespace (`package="…"`) parse the same way.
fn parse_attr(rest: &str) -> Option<(String, String)> {
    let eq = rest.find('=')?;
    let key = &rest[..eq];
    let key = key.split('(').next().unwrap_or(key);
    let name = key.rsplit(':').next().unwrap_or(key).to_string();
    let value = rest[eq + 1..].strip_prefix('"')?;
    let end = value.find('"')?;
    Some((name, value[..end].to_string()))
}

/// The SHA-256 of the one signer's certificate in `apksigner verify
/// --print-certs` output (`Signer #1 certificate SHA-256 digest: <hex>`).
fn signer_cert_sha256(output: &str) -> Result<[u8; 32]> {
    let digests: Vec<&str> = output
        .lines()
        .filter_map(|l| l.split_once("certificate SHA-256 digest:"))
        .filter(|(prefix, _)| prefix.trim_start().starts_with("Signer"))
        .map(|(_, hex)| hex.trim())
        .collect();
    let hex = match digests.as_slice() {
        [] => bail!("apksigner printed no signer certificate"),
        [one] => *one,
        _ => bail!("the APK has several signers; provisioning pins exactly one certificate"),
    };
    let mut out = [0u8; 32];
    if hex.len() != 64 {
        bail!("unexpected SHA-256 digest {hex:?}");
    }
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .with_context(|| format!("unexpected SHA-256 digest {hex:?}"))?;
    }
    Ok(out)
}

fn base64url_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = r#"N: android=http://schemas.android.com/apk/res/android (line=2)
  E: manifest (line=2)
    A: http://schemas.android.com/apk/res/android:versionCode(0x0101021b)=1002003
    A: http://schemas.android.com/apk/res/android:versionName(0x0101021c)="1.2.3" (Raw: "1.2.3")
    A: package="com.crewforgeos.agent" (Raw: "com.crewforgeos.agent")
      E: application (line=20)
        A: http://schemas.android.com/apk/res/android:label(0x01010001)="Agent" (Raw: "Agent")
          E: activity (line=30)
            A: http://schemas.android.com/apk/res/android:name(0x01010003)="com.crewforgeos.agent.MainActivity" (Raw: "com.crewforgeos.agent.MainActivity")
          E: receiver (line=50)
            A: http://schemas.android.com/apk/res/android:name(0x01010003)="io.idealyst.deviceadmin.BootReceiver" (Raw: "io.idealyst.deviceadmin.BootReceiver")
          E: receiver (line=60)
            A: http://schemas.android.com/apk/res/android:name(0x01010003)="io.idealyst.deviceadmin.AdminReceiver" (Raw: "io.idealyst.deviceadmin.AdminReceiver")
            A: http://schemas.android.com/apk/res/android:permission(0x01010006)="android.permission.BIND_DEVICE_ADMIN" (Raw: "android.permission.BIND_DEVICE_ADMIN")
            A: http://schemas.android.com/apk/res/android:exported(0x01010010)=(type 0x12)0xffffffff
"#;

    #[test]
    fn finds_the_bind_device_admin_receiver() {
        let (pkg, receiver) = device_admin_receiver(DUMP).unwrap();
        assert_eq!(pkg, "com.crewforgeos.agent");
        assert_eq!(receiver, "io.idealyst.deviceadmin.AdminReceiver");
    }

    #[test]
    fn an_app_without_a_device_admin_receiver_is_refused() {
        let dump = DUMP.replace("android.permission.BIND_DEVICE_ADMIN", "android.permission.X");
        let msg = format!("{:#}", device_admin_receiver(&dump).unwrap_err());
        assert!(msg.contains("BIND_DEVICE_ADMIN"), "{msg}");
    }

    #[test]
    fn a_relative_receiver_name_resolves_against_the_package() {
        let dump = DUMP.replace("\"io.idealyst.deviceadmin.AdminReceiver\"", "\".Admin\"");
        assert_eq!(device_admin_receiver(&dump).unwrap().1, "com.crewforgeos.agent.Admin");
    }

    const CERTS: &str = "Signer #1 certificate DN: CN=Android Debug, O=Android, C=US
Signer #1 certificate SHA-256 digest: 2fd4e1c67a2d28fced849ee1bb76e7391b93eb12d8d4c1c6e4e8f1b2a3c4d5e6
Signer #1 certificate SHA-1 digest: 0123456789abcdef0123456789abcdef01234567
";

    #[test]
    fn reads_the_signer_digest_and_encodes_the_checksum() {
        let digest = signer_cert_sha256(CERTS).unwrap();
        assert_eq!(digest[0], 0x2f);
        assert_eq!(digest[31], 0xe6);
        let info = ApkProvisioningInfo {
            package: "com.crewforgeos.agent".into(),
            receiver: "io.idealyst.deviceadmin.AdminReceiver".into(),
            cert_sha256: digest,
        };
        let checksum = info.signature_checksum();
        // 32 bytes → 43 base64 characters with the padding dropped, URL-safe.
        assert_eq!(checksum.len(), 43);
        assert!(!checksum.contains(['+', '/', '=']), "{checksum}");
        assert_eq!(checksum, "L9ThxnotKPzthJ7hu3bnORuT6xLY1MHG5OjxsqPE1eY");
    }

    #[test]
    fn several_signers_are_refused() {
        let two = format!("{CERTS}{}", CERTS.replace("#1", "#2"));
        assert!(signer_cert_sha256(&two).is_err());
    }

    #[test]
    fn base64url_matches_known_vectors() {
        assert_eq!(base64url_no_pad(b""), "");
        assert_eq!(base64url_no_pad(b"f"), "Zg");
        assert_eq!(base64url_no_pad(b"fo"), "Zm8");
        assert_eq!(base64url_no_pad(b"foo"), "Zm9v");
        assert_eq!(base64url_no_pad(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn payload_carries_component_download_checksum_and_extras() {
        let info = ApkProvisioningInfo {
            package: "com.crewforgeos.agent".into(),
            receiver: "io.idealyst.deviceadmin.AdminReceiver".into(),
            cert_sha256: [0u8; 32],
        };
        let extras = json!({ "server": "https://crewforgeos.com", "enrolment_token": "abc" });
        let p = payload(&info, "https://example.com/agent.apk", Some(&extras)).unwrap();
        assert_eq!(
            p["android.app.extra.PROVISIONING_DEVICE_ADMIN_COMPONENT_NAME"],
            "com.crewforgeos.agent/io.idealyst.deviceadmin.AdminReceiver"
        );
        assert_eq!(
            p["android.app.extra.PROVISIONING_DEVICE_ADMIN_PACKAGE_DOWNLOAD_LOCATION"],
            "https://example.com/agent.apk"
        );
        assert_eq!(
            p["android.app.extra.PROVISIONING_DEVICE_ADMIN_SIGNATURE_CHECKSUM"],
            info.signature_checksum()
        );
        assert_eq!(p["android.app.extra.PROVISIONING_ADMIN_EXTRAS_BUNDLE"]["server"], "https://crewforgeos.com");
        // Bundled apps are removed unless asked: the key is absent, not false.
        assert!(p.get("android.app.extra.PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED").is_none());
        let kept = payload_with(&info, "https://example.com/agent.apk", None, true).unwrap();
        assert_eq!(kept["android.app.extra.PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED"], true);

        assert!(payload(&info, "http://example.com/a.apk", None).is_err());
        assert!(payload(&info, "https://e.com/a.apk", Some(&json!(["x"]))).is_err());
        assert!(payload(&info, "https://e.com/a.apk", Some(&json!({ "k": null }))).is_err());
    }
}
