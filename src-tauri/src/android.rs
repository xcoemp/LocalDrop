//! # `android.rs` — Android platform lookups over JNI
//!
//! Compiled only for `target_os = "android"`; desktop builds get the stub
//! module in `lib.rs` instead. **A green `cargo test` on Windows says nothing
//! about this file** — run `pnpm cargo:android` after touching it.
//!
//! Every function here returns `Option` or degrades to a no-op rather than
//! propagating an error. That is the module's governing rule: these are
//! platform *enhancements* (a real filename, a wake lock, a notification), and
//! none of them is worth failing a transfer over.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>
//!
//! # Getting a JVM handle
//!
//! The handles come from tao's own registry via
//! `tao::platform::android::prelude::main_android_context()`, which yields the
//! `JavaVM` and the activity `jobject`. `tao` must therefore resolve to the same
//! version Tauri uses, so cargo unifies the crate and this reads the registry
//! the running app populated; a second copy would have its own empty statics and
//! silently return `None`.
//!
//! Do **not** substitute `ndk_context`: it is populated by `ndk-glue`, which
//! nothing in this stack uses, so `android_context()` panics at runtime while
//! compiling perfectly well.
//!
//! The context is `None` until the activity exists, so callers during early
//! startup get a fallback rather than a panic.
//!
//! # Pending exceptions abort the process
//!
//! A throwing Java call leaves the exception *pending* on the thread. The `jni`
//! crate surfaces an `Err`, but discarding it with `.ok()?` does not clear the
//! exception, and the next JNI call on that thread aborts the VM with `SIGABRT`
//! — far from the call that actually failed. Every entry point funnels through
//! [`with_env`], which clears before returning, and fallible steps are separated
//! by [`clear_pending`] so one failure cannot poison the next.

use jni::objects::{JObject, JString, JValue};
use jni::JNIEnv;

/// Run `f` with an attached JNI environment and the activity context.
///
/// Returns `None` if the activity does not exist yet, rather than panicking.
fn with_env<T>(f: impl FnOnce(&mut JNIEnv, &JObject) -> Option<T>) -> Option<T> {
    // tao re-exports its glue through `prelude`, not as a named submodule.
    //
    // `?` here is the early-startup case: the activity may not exist yet when
    // Rust setup runs, and every caller treats `None` as "not available" rather
    // than as an error. This is why AND-2's service is started from the
    // frontend's `onMounted` rather than during setup.
    let ctx = tao::platform::android::prelude::main_android_context()?;

    // SAFETY: both pointers come from tao's registry and are valid for as long
    // as the activity lives, which outlives any call made here.
    let vm = unsafe { jni::JavaVM::from_raw(ctx.java_vm.cast()) }.ok()?;
    let mut guard = vm.attach_current_thread().ok()?;
    let context = unsafe { JObject::from_raw(ctx.context_jobject.cast()) };

    let result = f(&mut guard, &context);

    // Never return to Rust with an exception pending.
    //
    // Note the result is *discarded* when an exception was found, even if `f`
    // produced a `Some`: a value computed on a thread with a pending exception
    // cannot be trusted, and returning it would hide the failure. See the
    // module docs — the next JNI call on this thread would otherwise abort the
    // VM with SIGABRT, far from the call that actually threw.
    if clear_pending(&mut guard) {
        return None;
    }
    result
}

/// Resolve one of the app's own Kotlin classes.
///
/// `FindClass` — which is what passing a string descriptor to `call_static_method`
/// uses — resolves against the *system* class loader on a thread attached from
/// native code. That loader only knows framework classes, so `android/net/Uri`
/// works while `app.localdrop.LocalDropService` fails with:
///
/// ```text
/// ClassNotFoundException: Didn't find class "app.localdrop.LocalDropService"
/// on path: DexPathList[[directory "."], nativeLibraryDirectories=[/system/lib64, …]]
/// ```
///
/// The activity's own class loader can see the APK's dex files, so app classes
/// must be looked up through it. `name` is dotted, e.g. `app.localdrop.Foo`.
fn find_app_class<'a>(
    env: &mut JNIEnv<'a>,
    context: &JObject,
    name: &str,
) -> Option<jni::objects::JClass<'a>> {
    // Three hops, each `?`-guarded: activity → its Class → its ClassLoader.
    // The chain has to start from the activity object because that is the only
    // thing in scope that was loaded from the APK, and therefore the only route
    // to a loader that can see the APK's other classes.
    //
    // `.ok()?` then `.l()` on each: the first unwraps the JNI call, the second
    // narrows the returned `JValue` to an object reference.
    let class = env
        .call_method(context, "getClass", "()Ljava/lang/Class;", &[])
        .ok()?
        .l()
        .ok()?;
    let loader = env
        .call_method(&class, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .ok()?
        .l()
        .ok()?;

    let name = env.new_string(name).ok()?;
    let loaded = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )
        .ok()?
        .l()
        .ok()?;

    Some(jni::objects::JClass::from(loaded))
}

/// Clear any pending Java exception. Returns whether one was found.
fn clear_pending(env: &mut JNIEnv) -> bool {
    match env.exception_check() {
        Ok(true) => {
            // Goes to logcat, so the underlying Java failure stays visible.
            // Described *before* clearing — the reverse order would discard the
            // stack trace and leave only the fact that something threw.
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            true
        }
        // `Ok(false)` and `Err(_)` collapsed deliberately: a failed
        // `exception_check` cannot be recovered from here, and reporting "no
        // exception" keeps the caller's result intact rather than discarding a
        // value over a diagnostic call that failed.
        _ => false,
    }
}

/// Reject a Java string that is blank or whitespace-only.
///
/// Android reports an empty `DEVICE_NAME` and an empty `DISPLAY_NAME` rather
/// than omitting them, so "present" is not the same as "usable" — this is the
/// same distinction `settings::usable` makes for hostnames.
fn non_empty(text: String) -> Option<String> {
    let trimmed = text.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Read a Java string into an owned `String`.
///
/// Bound to a `String` before returning because a `JavaStr` borrows the env,
/// which borrows the VM; as a tail expression that temporary outlives the VM
/// and fails borrow-check (E0597).
fn read_string(env: &mut JNIEnv, object: JObject) -> Option<String> {
    // Null-checked explicitly: Java returns null freely, and `get_string` on a
    // null reference is undefined behaviour rather than an error.
    if object.is_null() {
        return None;
    }
    let text: String = env.get_string(&JString::from(object)).ok()?.into();
    Some(text)
}

/// The real filename behind a `content://` URI, e.g. `IMG_20260915.jpg`.
///
/// This is the only way to name a media-provider pick: those URIs are opaque row
/// ids (`document/image%3A1000000034`) with no filename anywhere in them, so
/// without this the receiver can only invent `shared-<timestamp>`.
pub fn display_name(uri: &str) -> Option<String> {
    with_env(|env, context| {
        let uri_string = env.new_string(uri).ok()?;
        let uri_object = env
            .call_static_method(
                "android/net/Uri",
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&uri_string)],
            )
            .ok()?
            .l()
            .ok()?;

        let resolver = env
            .call_method(
                context,
                "getContentResolver",
                "()Landroid/content/ContentResolver;",
                &[],
            )
            .ok()?
            .l()
            .ok()?;

        // Can throw SecurityException if the grant has lapsed.
        let null = JObject::null();
        let cursor = env
            .call_method(
                &resolver,
                "query",
                "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
                &[
                    JValue::Object(&uri_object),
                    JValue::Object(&null),
                    JValue::Object(&null),
                    JValue::Object(&null),
                    JValue::Object(&null),
                ],
            )
            .ok()?
            .l()
            .ok()?;

        // A null cursor means the provider declined the query outright, with no
        // resource to release — returning here is correct, and skipping the
        // close below is why this is checked separately from the closure.
        if cursor.is_null() {
            return None;
        }

        // Wrapped in an immediately-invoked closure so its `?`s return from
        // *here*, not from `with_env`. That is what guarantees the cursor is
        // closed on every failure path below — the Java equivalent of a
        // try/finally, which JNI gives no other way to express.
        let name = (|| {
            let column = env.new_string("_display_name").ok()?; // OpenableColumns
            let index = env
                .call_method(
                    &cursor,
                    "getColumnIndex",
                    "(Ljava/lang/String;)I",
                    &[JValue::Object(&column)],
                )
                .ok()?
                .i()
                .ok()?;

            // `getColumnIndex` returns -1 rather than throwing when the column
            // is absent, which is the normal answer for a provider that does
            // not expose `OpenableColumns` at all.
            if index < 0 {
                return None;
            }
            // An empty result set: the URI resolved but addresses no row, e.g.
            // a file deleted since it was picked.
            if !env
                .call_method(&cursor, "moveToFirst", "()Z", &[])
                .ok()?
                .z()
                .ok()?
            {
                return None;
            }

            let value = env
                .call_method(
                    &cursor,
                    "getString",
                    "(I)Ljava/lang/String;",
                    &[JValue::Int(index)],
                )
                .ok()?
                .l()
                .ok()?;

            read_string(env, value).and_then(non_empty)
        })();

        // Clear before the next call: closing with an exception pending aborts
        // the process instead of failing.
        //
        // Three statements, in this exact order. The first clears anything the
        // closure above left pending; the close then runs on a clean thread;
        // the second clear handles the close itself throwing. Removing either
        // clear reintroduces the SIGABRT described in the module docs.
        clear_pending(env);
        let _ = env.call_method(&cursor, "close", "()V", &[]);
        clear_pending(env);

        name
    })
}

/// Start the foreground service, or update its notification text (AND-2).
///
/// Without it Android suspends the process when the app is backgrounded and the
/// peer sees `ERR_CONNECTION_LOST` partway through a transfer. Safe to call
/// repeatedly; a second call just refreshes the notification.
///
/// `detail` is the notification's body, e.g. `"Sending Project.zip"`. `None`
/// uses the idle listening text.
pub fn start_background_service(detail: Option<&str>) {
    with_env(|env, context| {
        // Two selections on the same `Option`, and both are needed. The first
        // allocates a Java string only when there is text; the second decides
        // whether to pass it or a null reference.
        //
        // The empty-string branch is never actually sent — it exists so `text`
        // has a value to bind in both arms, since a `JString` cannot be
        // conditionally uninitialised. Passing null (rather than "") is what
        // lets Kotlin apply its own default idle text.
        let text = match detail {
            Some(d) => env.new_string(d).ok()?,
            // A null String argument makes Kotlin apply its own default.
            None => env.new_string("").ok()?,
        };
        let arg: JObject = if detail.is_some() {
            text.into()
        } else {
            JObject::null()
        };

        let class = find_app_class(env, context, "app.localdrop.LocalDropService")?;
        env.call_static_method(
            &class,
            "start",
            "(Landroid/content/Context;Ljava/lang/String;)V",
            &[JValue::Object(context), JValue::Object(&arg)],
        )
        .ok()?;
        Some(())
    });
}

/// Stop the foreground service and release its wake lock.
pub fn stop_background_service() {
    with_env(|env, context| {
        let class = find_app_class(env, context, "app.localdrop.LocalDropService")?;
        env.call_static_method(
            &class,
            "stop",
            "(Landroid/content/Context;)V",
            &[JValue::Object(context)],
        )
        .ok()?;
        Some(())
    });
}

/// Whether the app is exempt from Doze battery optimisation.
///
/// `false` means long transfers can still be frozen between Doze maintenance
/// windows even with the foreground service running.
pub fn is_ignoring_battery_optimizations() -> bool {
    with_env(|env, context| {
        let class = find_app_class(env, context, "app.localdrop.LocalDropService")?;
        env.call_static_method(
            &class,
            "isIgnoringBatteryOptimizations",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(context)],
        )
        .ok()?
        .z()
        .ok()
    })
    // Defaults to `false` — "not exempt" — when the call fails or the activity
    // does not exist. Pessimistic on purpose: the Settings UI then offers the
    // exemption prompt, which is harmless if it was already granted, whereas
    // defaulting to `true` would hide a real cause of stalled transfers.
    .unwrap_or(false)
}

/// Open the system prompt to exempt LocalDrop from battery optimisation.
///
/// Call only from a user-initiated action: the dialog is disruptive, and Play
/// restricts apps that request the exemption without a qualifying use case.
pub fn request_ignore_battery_optimizations() {
    with_env(|env, context| {
        let class = find_app_class(env, context, "app.localdrop.LocalDropService")?;
        env.call_static_method(
            &class,
            "requestIgnoreBatteryOptimizations",
            "(Landroid/content/Context;)V",
            &[JValue::Object(context)],
        )
        .ok()?;
        Some(())
    });
}

/// Hold the screen on for the duration of a transfer.
///
/// A multi-hundred-megabyte transfer outlasts the display timeout, and once the
/// screen sleeps Android may suspend the app mid-stream — the peer then sees
/// `ERR_CONNECTION_LOST` with nothing in the log explaining it.
///
/// The Kotlin side performs the UI-thread hop, because window flags may only be
/// touched there and every caller here is a Tokio worker.
pub fn set_keep_screen_on(on: bool) {
    with_env(|env, context| {
        let class = find_app_class(env, context, "app.localdrop.LocalDropScreen")?;
        env.call_static_method(
            &class,
            "keepAwake",
            "(Landroid/app/Activity;Z)V",
            &[JValue::Object(context), JValue::Bool(u8::from(on))],
        )
        .ok()?;
        Some(())
    });
}

/// The device's user-facing name, e.g. "Paul's A16".
///
/// Prefers the name from Settings > About > Device name. Falls back to the
/// product model, which is also what is used when this runs before the activity
/// exists — `default_alias()` is evaluated during startup, so `None` here is
/// normal rather than a failure.
pub fn device_name() -> Option<String> {
    let from_settings = with_env(|env, context| {
        let resolver = env
            .call_method(
                context,
                "getContentResolver",
                "()Landroid/content/ContentResolver;",
                &[],
            )
            .ok()?
            .l()
            .ok()?;
        let key = env.new_string("device_name").ok()?;
        let value = env
            .call_static_method(
                "android/provider/Settings$Global",
                "getString",
                "(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&resolver), JValue::Object(&key)],
            )
            .ok()?
            .l()
            .ok()?;

        read_string(env, value).and_then(non_empty)
    });

    // Two-stage fallback. The Settings value is preferred because the user
    // chose it, and it is what they will recognise on another device's radar.
    if from_settings.is_some() {
        return from_settings;
    }

    // Reads the product model without touching the JVM, so it works at startup.
    // This is the branch that actually runs for `default_alias()`, which is
    // evaluated before the activity exists — hence `None` above being routine
    // rather than a failure worth logging.
    non_empty(whoami::devicename())
}
