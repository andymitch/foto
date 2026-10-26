foto_core::uniffi_reexport_scaffolding!();

#[cfg(target_os = "android")]
static ANDROID_INIT: std::sync::Once = std::sync::Once::new();

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_andymitch_foto_Native_initAndroidContext(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
    context: jni::objects::JObject,
) {
    ANDROID_INIT.call_once(|| {
        let Ok(vm) = env.get_java_vm() else { return };
        let Ok(global) = env.new_global_ref(&context) else { return };
        unsafe {
            ndk_context::initialize_android_context(
                vm.get_java_vm_pointer().cast(),
                global.as_obj().as_raw().cast(),
            );
        }
        std::mem::forget(global);
    });
}
