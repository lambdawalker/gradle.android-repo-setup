package com.apexfission.release

import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.api.tasks.Exec
import org.gradle.kotlin.dsl.register

class AndroidRepoSetupRootPlugin : Plugin<Project> {
    override fun apply(project: Project) {
        require(project == project.rootProject) { "Apply the release root plugin to the root project" }
        for ((taskName, command) in listOf("generateImportDocs" to "generate", "verifyImportDocs" to "verify")) {
            project.tasks.register<Exec>(taskName) {
                group = "documentation"
                workingDir(project.rootDir)
                commandLine("android-repo-setup", "module_release", command)
            }
        }
    }
}
