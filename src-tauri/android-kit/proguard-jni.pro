# proguard-jni.pro -- keep the Kotlin methods that Rust calls over JNI.
#
# Project: LocalDrop -- zero-configuration LAN file and text transfer
# Author:  Emmanuel Paul <pauldukz@gmail.com>
#
# ---------------------------------------------------------------------------
# Why this file exists
# ---------------------------------------------------------------------------
#
# `isMinifyEnabled = true` on the release build type, so R8 tree-shakes the dex.
# R8 reasons only about Java/Kotlin call graphs. Every method in this file is
# reached exclusively from Rust through `JNIEnv::call_static_method`, which is an
# invisible edge as far as R8 is concerned -- nothing in Kotlin calls them, so
# they are dead code and it deletes them.
#
# The rule that ships with wry does not cover this:
#
#     -keep class app.localdrop.* {
#       native <methods>;
#     }
#
# That keeps the class *names* but only their `native` members. Our classes have
# no native methods -- they are called *from* native -- so every one of their
# members was discarded.
#
# ---------------------------------------------------------------------------
# What it looked like when this was missing
# ---------------------------------------------------------------------------
#
# The build succeeded. The APK installed, launched, discovered peers and
# transferred files, so nothing looked wrong. What silently did nothing:
#
#   * Settings > "Battery optimisation is on" > Allow -- the button did nothing,
#     because `requestIgnoreBatteryOptimizations` was gone.
#   * Transfers dropped when the screen locked -- `LocalDropService.start` and
#     `LocalDropScreen.keepAwake` were gone, so neither the foreground service
#     nor the wake lock was ever taken, and Android suspended the process.
#   * The background-listener toggle reported success and started nothing.
#
# Debug builds were unaffected (`isMinifyEnabled = false`), which is the trap:
# `tauri android dev` exercises none of this, so the whole class of failure is
# invisible until someone installs a release build.
#
# The failure mode is a silent no-op rather than a crash: `android.rs` routes
# every call through `with_env`, which logs the Java exception to logcat and
# clears it. So the evidence is a `NoSuchMethodError` in logcat and nothing at
# all in the UI.
#
# Verify a build kept them with R8's own reports, which are the authority here:
#
#   gen/android/app/build/outputs/mapping/universalRelease/usage.txt   <- deleted
#   gen/android/app/build/outputs/mapping/universalRelease/seeds.txt   <- kept
#
# If any LocalDrop* method appears in usage.txt, this file is not being applied.
#
# ---------------------------------------------------------------------------
# Scope
# ---------------------------------------------------------------------------
#
# `{ *; }` rather than naming each signature. These are four small glue classes
# whose entire purpose is to be called from Rust, so there is nothing here worth
# shrinking, and an exact-signature list would be one more place to forget to
# update -- the failure it would cause being another silent no-op in release
# only. Keeping them whole costs a few hundred bytes of dex.
#
# The `$*` companions matter for LocalDropService: `@JvmStatic` inside a
# `companion object` emits the static on the outer class *and* an instance
# method on `LocalDropService$Companion`, and R8 removed both.

# LocalDropService -- AND-2 foreground service, wake lock, and the
# battery-optimisation exemption prompt.
#   Rust: start, stop, isIgnoringBatteryOptimizations,
#         requestIgnoreBatteryOptimizations
-keep class app.localdrop.LocalDropService { *; }
-keep class app.localdrop.LocalDropService$* { *; }

# LocalDropScreen -- FLAG_KEEP_SCREEN_ON while a transfer is in flight.
#   Rust: keepAwake
-keep class app.localdrop.LocalDropScreen { *; }
-keep class app.localdrop.LocalDropScreen$* { *; }

# LocalDropNetwork -- AND-1 multicast lock, without which inbound UDP discovery
# is silently dropped by Android power management.
#   Called from MainActivity, but kept explicitly: whether R8 can see that edge
#   should not decide whether discovery works.
-keep class app.localdrop.LocalDropNetwork { *; }
-keep class app.localdrop.LocalDropNetwork$* { *; }

# LocalDropStorage -- MANAGE_EXTERNAL_STORAGE state and its settings intent.
#   Rust: hasAllFilesAccess, requestAllFilesAccess
-keep class app.localdrop.LocalDropStorage { *; }
-keep class app.localdrop.LocalDropStorage$* { *; }

# Kotlin `object` singletons are reached through their INSTANCE field, and the
# static initialiser is what populates it. Covered by `{ *; }` above, but stated
# so that narrowing those rules later does not quietly break the singletons.
-keepclassmembers class app.localdrop.LocalDrop** {
    public static ** INSTANCE;
    static <clinit>();
}
