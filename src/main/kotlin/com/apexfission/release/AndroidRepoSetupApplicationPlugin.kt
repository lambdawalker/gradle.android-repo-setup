package com.apexfission.release

import com.android.build.api.variant.ApplicationAndroidComponentsExtension
import org.gradle.api.Action
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.kotlin.dsl.getByType

/**
 * Shared conventions for runnable Android applications.
 *
 * Secret persistence remains owned by the android-repo-setup CLI. This plugin
 * only consumes process/local build inputs supplied for the current Gradle run.
 */
class AndroidRepoSetupApplicationPlugin : Plugin<Project> {
    override fun apply(project: Project) {
        project.pluginManager.apply("com.android.application")
        val conventions = project.extensions.create(
            "androidRepoSetup",
            AndroidRepoSetupApplicationExtension::class.java,
        )
        project.extensions.getByType<ApplicationAndroidComponentsExtension>().finalizeDsl { android ->
            project.configureEndpointConventions(android, conventions.endpoints)
            project.configureSigningConventions(android, conventions.signing)
        }
    }
}

open class AndroidRepoSetupApplicationExtension {
    val signing = AndroidSigningConvention()
    val endpoints = AndroidEndpointsConvention()

    fun signing(action: Action<AndroidSigningConvention>) = action.execute(signing)
    fun endpoints(action: Action<AndroidEndpointsConvention>) = action.execute(endpoints)
}
