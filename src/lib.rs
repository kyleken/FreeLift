//! Free Lift for the Gym Monster tablet (Android only).

pub mod app;
pub mod commands;
pub mod machine;
pub mod machine_link;
pub mod mechanics;
pub mod protocol;
pub mod serial_link;
pub mod session;
pub mod voice;

#[cfg(target_os = "android")]
static ANDROID_APP: std::sync::OnceLock<winit::platform::android::activity::AndroidApp> = std::sync::OnceLock::new();

/// Open Android's Settings screen (Intent "android.settings.SETTINGS", new task).
#[cfg(target_os = "android")]
pub fn open_android_settings() -> Result<(), String> {
    start_settings_intent("android.settings.SETTINGS", None)
}

/// Open the App info page of another app (where "Force stop" is).
#[cfg(target_os = "android")]
pub fn open_app_info(package: &str) -> Result<(), String> {
    start_settings_intent("android.settings.APPLICATION_DETAILS_SETTINGS", Some(&format!("package:{package}")))
}

#[cfg(target_os = "android")]
fn start_settings_intent(action_name: &str, data_uri: Option<&str>) -> Result<(), String> {
    use jni::objects::JValue;
    with_activity(|env, activity| {
        let action = env.new_string(action_name)?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action)])?;
        if let Some(uri) = data_uri {
            let s = env.new_string(uri)?;
            let parsed = env
                .call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&s)])?
                .l()?;
            env.call_method(&intent, "setData", "(Landroid/net/Uri;)Landroid/content/Intent;", &[JValue::Object(&parsed)])?;
        }
        // FLAG_ACTIVITY_NEW_TASK
        env.call_method(&intent, "addFlags", "(I)Landroid/content/Intent;", &[JValue::Int(0x1000_0000)])?;
        env.call_method(activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    })
}

/// Run `f` with the JNI environment and the activity; Java exceptions are cleared and turned
/// into an error.
#[cfg(target_os = "android")]
fn with_activity<T>(
    f: impl FnOnce(&mut jni::JNIEnv, &jni::objects::JObject) -> jni::errors::Result<T>,
) -> Result<T, String> {
    use jni::objects::JObject;
    let app = ANDROID_APP.get().ok_or("app not ready")?;
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    // the activity is a global reference owned by android-activity; JObject doesn't free it
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    let result = f(&mut env, &activity);
    if result.is_err() {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result.map_err(|e| e.to_string())
}

#[cfg(target_os = "android")]
const RECORD_AUDIO: &str = "android.permission.RECORD_AUDIO";

/// Free Lift may use the microphone (Context.checkSelfPermission == PERMISSION_GRANTED).
#[cfg(target_os = "android")]
pub fn has_mic_permission() -> Result<bool, String> {
    with_activity(|env, activity| {
        let perm = env.new_string(RECORD_AUDIO)?;
        let r = env
            .call_method(activity, "checkSelfPermission", "(Ljava/lang/String;)I", &[jni::objects::JValue::Object(&perm)])?
            .i()?;
        Ok(r == 0)
    })
}

/// Show Android's "Allow Free Lift to record audio?" question (Activity.requestPermissions).
#[cfg(target_os = "android")]
pub fn request_mic_permission() -> Result<(), String> {
    with_activity(|env, activity| {
        let perm = env.new_string(RECORD_AUDIO)?;
        let arr = env.new_object_array(1, "java/lang/String", &perm)?;
        env.call_method(
            activity,
            "requestPermissions",
            "([Ljava/lang/String;I)V",
            &[jni::objects::JValue::Object(&arr), jni::objects::JValue::Int(7)],
        )?;
        Ok(())
    })
}

/// The speech model's folder in the app's own storage. The files ship inside the APK
/// (assets/voice/en-20M) and are copied out once (again only if a file's size changed),
/// because the recogniser reads ordinary files.
#[cfg(target_os = "android")]
pub fn voice_model_dir() -> Result<std::path::PathBuf, String> {
    let app = ANDROID_APP.get().ok_or("app not ready")?;
    let base = app.internal_data_path().ok_or("no app folder")?.join("voice").join("en-20M");
    std::fs::create_dir_all(&base).map_err(|e| format!("can't make {}: {e}", base.display()))?;
    let assets = app.asset_manager();
    for name in voice::MODEL_FILES {
        let path = std::ffi::CString::new(format!("voice/en-20M/{name}")).map_err(|e| e.to_string())?;
        let mut asset = assets.open(&path).ok_or_else(|| format!("speech model file {name} is missing from the app"))?;
        let dst = base.join(name);
        if std::fs::metadata(&dst).map(|m| m.len() == asset.length() as u64).unwrap_or(false) {
            continue;
        }
        let tmp = base.join(format!("{name}.part"));
        let mut copy = || -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            std::io::copy(&mut asset, &mut f)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &dst)
        };
        copy().map_err(|e| format!("couldn't copy the speech model ({name}): {e}"))?;
    }
    Ok(base)
}

/// Android entry point (NativeActivity). Full screen; settings go to the app's private folder.
#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    let _ = ANDROID_APP.set(android_app.clone());
    if let Some(dir) = android_app.internal_data_path() {
        let _ = app::DATA_DIR.set(dir);
    }
    let options = eframe::NativeOptions { android_app: Some(android_app), ..Default::default() };
    let _ = eframe::run_native("Free Lift", options, Box::new(|cc| Ok(Box::new(app::FreeLiftApp::new(cc)))));
}
