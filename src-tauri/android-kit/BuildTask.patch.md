# Patch: `buildSrc/.../kotlin/BuildTask.kt`

**Required after every `tauri android init`.** `gen/android/` is generated and
not committed, so this is lost whenever the project is regenerated.

Simplest application: copy [`BuildTask.kt`](BuildTask.kt) from this directory
over `gen/android/buildSrc/src/main/java/app/localdrop/kotlin/BuildTask.kt`.
It carries both fixes below. Note the file has **no `package` declaration** —
the generated one does not either, despite its path.

---

## Fix 2: Gradle 9 incompatibility

### Symptom (Gradle 9)

Every build ends with:

```text
Deprecated Gradle features were used in this build, making it incompatible
with Gradle 9.0.
```

Harmless on Gradle 8.14.3 (what the wrapper pins), fatal on Gradle 9.

### Cause (Gradle 9)

The generated task calls `project.exec { … }` inside its `@TaskAction`.
`Project.exec` was deprecated in Gradle 8.11 and **removed in 9.0**. Reading
`project.projectDir` and `project.logger` at execution time is the same class of
problem, and is what prevents the configuration cache from working.

`--warning-mode all` on its own will not show this: it only appears when the
task actually executes, so `tasks --dry-run` reports nothing. The other warning
you will see there — `StartParameter.isConfigurationCacheRequested` — comes from
the Android Gradle Plugin, targets Gradle 10, and is not actionable here.

### Fix (Gradle 9)

Inject `ExecOperations` and `ProjectLayout` instead of reaching for `Project`:

```kotlin
abstract class BuildTask @Inject constructor(
    private val execOperations: ExecOperations,
    private val projectLayout: ProjectLayout,
) : DefaultTask() {
    // …
    execOperations.exec {
        workingDir(File(projectLayout.projectDirectory.asFile, rootDirRel))
        // `logger` is the task's own, not the project's
    }.assertNormalExitValue()
}
```

`open class` becomes `abstract class`: `RustPlugin` creates the task with
`maybeCreate(..., BuildTask::class.java)`, so Gradle generates the
implementation and supplies the injected services.

---

## Fix 1: CLI not found under pnpm

## Symptom

The Rust library builds successfully and is linked into `jniLibs`, then Gradle
fails immediately afterwards:

```text
Error: Cannot find module 'C:\...\src-tauri\tauri'
  code: 'MODULE_NOT_FOUND'
...
Execution failed for task ':app:rustBuildArm64Release'.
> A problem occurred starting process 'command 'node.bat''
```

The real error is buried under a few hundred lines of serialized environment
variables, and the `node.bat` mention points at the wrong thing entirely.

## Cause

The generated task shells out to:

```kotlin
val executable = """node"""
val args = listOf("tauri", "android", "android-studio-script")
```

with `workingDir` set to `src-tauri`. That runs `node tauri …`, so Node looks
for a **module** named `tauri` inside `src-tauri/` — which does not exist. The
template assumes an npm-style flat `node_modules`; it does not resolve the CLI
under pnpm, and this project invokes Tauri through `vp exec` besides.

## Fix

In `gen/android/buildSrc/src/main/java/app/localdrop/kotlin/BuildTask.kt`:

```kotlin
// was: val executable = """node"""
val executable = """pnpm"""

// was: val args = listOf("tauri", "android", "android-studio-script")
val args = listOf("exec", "tauri", "android", "android-studio-script")
```

`pnpm exec` walks up from `workingDir` (`src-tauri`) to the repo root and finds
the CLI in `node_modules/.bin`. The task's existing Windows fallback appends
`.cmd`/`.bat` to the executable name, so `pnpm` resolves correctly there too.

Leave `workingDir` alone — the Tauri CLI expects to run from `src-tauri`.
