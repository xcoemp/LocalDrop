import java.io.File
import javax.inject.Inject
import org.apache.tools.ant.taskdefs.condition.Os
import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.ProjectLayout
import org.gradle.api.logging.LogLevel
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

/**
 * Replacement for the generated
 * `gen/android/buildSrc/src/main/java/<package>/kotlin/BuildTask.kt`.
 *
 * Two changes from the Tauri template — see BuildTask.patch.md:
 *
 *  1. Invokes the Tauri CLI through `pnpm exec` rather than `node tauri`, which
 *     does not resolve under pnpm and fails with MODULE_NOT_FOUND.
 *
 *  2. Uses injected `ExecOperations` / `ProjectLayout` instead of
 *     `project.exec {}` and `project.projectDir`. `Project.exec` was deprecated
 *     in Gradle 8.11 and **removed in Gradle 9.0** — it is the source of
 *     "Deprecated Gradle features were used in this build, making it
 *     incompatible with Gradle 9.0". Reaching for `Task.project` during task
 *     execution is also what blocks the configuration cache.
 */
abstract class BuildTask
@Inject
constructor(
    private val execOperations: ExecOperations,
    private val projectLayout: ProjectLayout,
) : DefaultTask() {
    @Input var rootDirRel: String? = null

    @Input var target: String? = null

    @Input var release: Boolean? = null

    @TaskAction
    fun assemble() {
        val executable = """pnpm"""
        try {
            runTauriCli(executable)
        } catch (e: Exception) {
            if (Os.isFamily(Os.FAMILY_WINDOWS)) {
                // Try different Windows-specific extensions
                val fallbacks =
                    listOf(
                        "$executable.exe",
                        "$executable.cmd",
                        "$executable.bat",
                    )

                var lastException: Exception = e
                for (fallback in fallbacks) {
                    try {
                        runTauriCli(fallback)
                        return
                    } catch (fallbackException: Exception) {
                        lastException = fallbackException
                    }
                }
                throw lastException
            } else {
                throw e
            }
        }
    }

    fun runTauriCli(executable: String) {
        val rootDirRel = rootDirRel ?: throw GradleException("rootDirRel cannot be null")
        val target = target ?: throw GradleException("target cannot be null")
        val release = release ?: throw GradleException("release cannot be null")

        // `pnpm exec` walks up from workingDir (src-tauri) to the repo root and
        // finds the CLI in node_modules/.bin.
        val args = listOf("exec", "tauri", "android", "android-studio-script")

        execOperations
            .exec {
                workingDir(File(projectLayout.projectDirectory.asFile, rootDirRel))
                executable(executable)
                args(args)
                // `logger` belongs to the task, so no Project access at
                // execution time.
                if (logger.isEnabled(LogLevel.DEBUG)) {
                    args("-vv")
                } else if (logger.isEnabled(LogLevel.INFO)) {
                    args("-v")
                }
                if (release) {
                    args("--release")
                }
                args(listOf("--target", target))
            }
            .assertNormalExitValue()
    }
}
