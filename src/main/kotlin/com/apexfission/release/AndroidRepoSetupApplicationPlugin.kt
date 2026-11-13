package com.apexfission.release

import com.android.build.api.artifact.SingleArtifact
import com.android.build.api.variant.ApplicationAndroidComponentsExtension
import com.android.apksig.ApkVerifier
import org.gradle.api.Action
import org.gradle.api.GradleException
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
        project.tasks.register("verifyLocalSigning") {
            group = "verification"
            description = "Validates the configured local Android keystore and prints its SHA-256 certificate fingerprint."
            doLast {
                project.verifyLocalSigning(conventions.signing)
            }
        }

        val androidComponents =
            project.extensions.getByType<ApplicationAndroidComponentsExtension>()
        androidComponents.finalizeDsl { android ->
            project.configureEndpointConventions(android, conventions.endpoints)
            project.configureSigningConventions(android, conventions.signing)
        }
        androidComponents.onVariants { variant ->
            if (!conventions.signing.createSignedDebug ||
                variant.name != conventions.signing.signedDebugName
            ) {
                return@onVariants
            }

            val apkDirectory = variant.artifacts.get(SingleArtifact.APK)
            val taskName = "verify${variant.name.replaceFirstChar { it.uppercaseChar() }}Signing"
            project.tasks.register(taskName) {
                group = "verification"
                description =
                    "Verifies the ${variant.name} APK signer matches the configured local Android keystore."
                inputs.dir(apkDirectory)
                doLast {
                    val expected = project.localSigningVerification(conventions.signing)
                    val directory = apkDirectory.get().asFile
                    val apks = directory
                        .walkTopDown()
                        .filter { it.isFile && it.extension.equals("apk", ignoreCase = true) }
                        .sortedBy { it.absolutePath }
                        .toList()
                    if (apks.isEmpty()) {
                        throw GradleException(
                            "No APK was produced for variant '${variant.name}' in ${directory.absolutePath}"
                        )
                    }

                    apks.forEach { apk ->
                        val result = ApkVerifier.Builder(apk).build().verify()
                        if (!result.isVerified) {
                            throw GradleException(
                                "APK signature verification failed for '${apk.absolutePath}'"
                            )
                        }
                        val fingerprints = result.signerCertificates
                            .map(::certificateFingerprint)
                            .distinct()
                            .sorted()
                        if (fingerprints != listOf(expected.fingerprint)) {
                            throw GradleException(
                                "APK signer mismatch for '${apk.absolutePath}'. " +
                                    "Expected ${expected.fingerprint} from " +
                                    "${expected.keystore.absolutePath} alias ${expected.alias}; " +
                                    "APK signer(s): ${fingerprints.joinToString()}"
                            )
                        }
                        project.logger.lifecycle("APK signing verification: PASS")
                        project.logger.lifecycle("  Variant: ${variant.name}")
                        project.logger.lifecycle("  APK: ${apk.absolutePath}")
                        project.logger.lifecycle("  SHA-256: ${expected.fingerprint}")
                        project.logger.lifecycle(
                            "  Matches keystore: ${expected.keystore.absolutePath} " +
                                "(alias ${expected.alias})"
                        )
                    }
                }
            }
        }
    }
}

open class AndroidRepoSetupApplicationExtension {
    val signing = AndroidSigningConvention()
    val endpoints = AndroidEndpointsConvention()

    fun signing(action: Action<AndroidSigningConvention>) = action.execute(signing)
    fun endpoints(action: Action<AndroidEndpointsConvention>) = action.execute(endpoints)
}
