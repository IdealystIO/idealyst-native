//! `SharedPreferences`-backed plaintext store for Android, via JNI.
//!
//! Each store is its own `SharedPreferences` file (named by the
//! namespace), so isolation + `clear()` come for free from the platform.
//! SharedPreferences is a plaintext XML file in the app's data dir — never
//! put secrets here (see crate docs).
//!
//! JNI work runs on the calling thread (attached per op via the host
//! `JavaVM` from `ndk_context`, mirroring the `net`/`microphone` SDKs).
//! SharedPreferences reads/writes are cheap, so per-op attach is fine.

use jni::objects::{JObject, JValue};
use jni::JavaVM;

use crate::{Storage, StorageError, StorageFuture};

const MODE_PRIVATE: i32 = 0; // Context.MODE_PRIVATE

/// A [`Storage`] over a named `SharedPreferences` file.
pub struct SharedPrefsStorage {
    name: String,
}

impl SharedPrefsStorage {
    pub fn new(namespace: &str) -> Self {
        Self {
            name: namespace.to_string(),
        }
    }
}

fn java_vm() -> Result<JavaVM, StorageError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| StorageError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn map_jni(e: jni::errors::Error) -> StorageError {
    StorageError::Backend(format!("JNI: {e}"))
}

/// `context.getSharedPreferences(name, MODE_PRIVATE)`.
fn open_prefs<'a>(
    env: &mut jni::JNIEnv<'a>,
    name: &str,
) -> Result<JObject<'a>, StorageError> {
    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    let name_j = env.new_string(name).map_err(map_jni)?;
    env.call_method(
        &activity,
        "getSharedPreferences",
        "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
        &[
            JValue::Object(&JObject::from(name_j)),
            JValue::Int(MODE_PRIVATE),
        ],
    )
    .map_err(map_jni)?
    .l()
    .map_err(map_jni)
}

/// `prefs.edit()` → apply `f` to the editor → `editor.apply()`.
fn edit<'a>(
    env: &mut jni::JNIEnv<'a>,
    prefs: &JObject<'a>,
    f: impl FnOnce(&mut jni::JNIEnv<'a>, &JObject<'a>) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    let editor = env
        .call_method(prefs, "edit", "()Landroid/content/SharedPreferences$Editor;", &[])
        .map_err(map_jni)?
        .l()
        .map_err(map_jni)?;
    f(env, &editor)?;
    // apply() commits asynchronously to disk but is immediately visible to
    // subsequent reads of the same prefs object.
    env.call_method(&editor, "apply", "()V", &[])
        .map_err(map_jni)?;
    Ok(())
}

impl Storage for SharedPrefsStorage {
    fn get(&self, key: &str) -> StorageFuture<'_, Option<String>> {
        let name = self.name.clone();
        let key = key.to_string();
        Box::pin(async move {
            let vm = java_vm()?;
            let mut env = vm.attach_current_thread().map_err(map_jni)?;
            let prefs = open_prefs(&mut env, &name)?;
            let key_j = env.new_string(&key).map_err(map_jni)?;
            // getString(key, null) → null when absent.
            let value = env
                .call_method(
                    &prefs,
                    "getString",
                    "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                    &[
                        JValue::Object(&JObject::from(key_j)),
                        JValue::Object(&JObject::null()),
                    ],
                )
                .map_err(map_jni)?
                .l()
                .map_err(map_jni)?;
            if value.is_null() {
                Ok(None)
            } else {
                let s: String = env.get_string(&value.into()).map_err(map_jni)?.into();
                Ok(Some(s))
            }
        })
    }

    fn set(&self, key: &str, value: &str) -> StorageFuture<'_, ()> {
        let name = self.name.clone();
        let key = key.to_string();
        let value = value.to_string();
        Box::pin(async move {
            let vm = java_vm()?;
            let mut env = vm.attach_current_thread().map_err(map_jni)?;
            let prefs = open_prefs(&mut env, &name)?;
            edit(&mut env, &prefs, |env, editor| {
                let key_j = env.new_string(&key).map_err(map_jni)?;
                let val_j = env.new_string(&value).map_err(map_jni)?;
                env.call_method(
                    editor,
                    "putString",
                    "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",
                    &[
                        JValue::Object(&JObject::from(key_j)),
                        JValue::Object(&JObject::from(val_j)),
                    ],
                )
                .map_err(map_jni)?;
                Ok(())
            })
        })
    }

    fn remove(&self, key: &str) -> StorageFuture<'_, ()> {
        let name = self.name.clone();
        let key = key.to_string();
        Box::pin(async move {
            let vm = java_vm()?;
            let mut env = vm.attach_current_thread().map_err(map_jni)?;
            let prefs = open_prefs(&mut env, &name)?;
            edit(&mut env, &prefs, |env, editor| {
                let key_j = env.new_string(&key).map_err(map_jni)?;
                env.call_method(
                    editor,
                    "remove",
                    "(Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",
                    &[JValue::Object(&JObject::from(key_j))],
                )
                .map_err(map_jni)?;
                Ok(())
            })
        })
    }

    fn clear(&self) -> StorageFuture<'_, ()> {
        let name = self.name.clone();
        Box::pin(async move {
            let vm = java_vm()?;
            let mut env = vm.attach_current_thread().map_err(map_jni)?;
            let prefs = open_prefs(&mut env, &name)?;
            edit(&mut env, &prefs, |env, editor| {
                env.call_method(
                    editor,
                    "clear",
                    "()Landroid/content/SharedPreferences$Editor;",
                    &[],
                )
                .map_err(map_jni)?;
                Ok(())
            })
        })
    }
}
