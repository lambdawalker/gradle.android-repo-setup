package com.apexfission.release

import com.vanniktech.maven.publish.AndroidSingleVariantLibrary
import com.vanniktech.maven.publish.JavadocJar
import com.vanniktech.maven.publish.MavenPublishBaseExtension
import com.vanniktech.maven.publish.SourcesJar
import java.util.Properties
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.api.publish.PublishingExtension
import org.gradle.api.tasks.Exec
import org.gradle.api.tasks.bundling.Jar
import org.gradle.kotlin.dsl.configure
import org.gradle.kotlin.dsl.register
import org.gradle.kotlin.dsl.withType

class AndroidRepoSetupPlugin : Plugin<Project> {
    override fun apply(project: Project) {
        require(project.pluginManager.hasPlugin("com.android.library")) {
            "Apply com.android.library before com.apexfission.android-repo-setup"
        }
        project.pluginManager.apply("com.vanniktech.maven.publish.base")
        project.configureRelease()
    }

    private fun Project.configureRelease() {
        val requestedVersion = providers.gradleProperty("releaseVersion").orNull
        val requestedModule = providers.gradleProperty("releaseModule").orNull
        require(requestedVersion == null || !requestedModule.isNullOrBlank()) {
            "Set -PreleaseModule when specifying -PreleaseVersion"
        }
        require(requestedVersion == null || Regex(STABLE_VERSION).matches(requestedVersion)) {
            "releaseVersion must be a stable semantic version such as 0.1.0"
        }
        val nativeJitPack = if (providers.environmentVariable("JITPACK").orNull == "true") {
            JitPackRelease.parse(providers.environmentVariable("VERSION").getOrElse(""))
        } else null
        val selectedModule = nativeJitPack?.module ?: requestedModule
        val selected = selectedModule == name
        val version = (nativeJitPack?.version ?: requestedVersion).takeIf { selected }
        val verificationJitPack = providers.gradleProperty("jitpackBuild").orNull == "true"
        val jitpack = selected && (nativeJitPack != null || verificationJitPack)
        require(!verificationJitPack || requestedVersion != null &&
            providers.gradleProperty("jitpackScopedTag").orNull == "true") {
            "A destination-scoped JitPack release identity is required"
        }
        val destination = providers.environmentVariable("RELEASE_REPOSITORY").getOrElse("maven-central")
        val repositoryConfig = rootProject.layout.projectDirectory.file(".project/setup/repositories.properties")
        val publisher = providers.fileContents(repositoryConfig).asText.orNull?.let { contents ->
            Properties().apply { contents.reader().use(::load) }.getProperty("$destination.publisher")
        } ?: when (destination) {
            "maven-central" -> "central"
            "jitpack" -> "jitpack"
            else -> null
        }
        require(publisher == "central" || publisher == "jitpack") { "Unknown release destination: $destination" }
        val centralVersion = version.takeIf { !jitpack && publisher == "central" }
        val moduleName = name
        val artifactId = providers.gradleProperty("POM_ARTIFACT_ID.$moduleName")
            .orElse(providers.gradleProperty("POM_ARTIFACT_ID"))
            .getOrElse(moduleName)

        extensions.configure<MavenPublishBaseExtension> {
            coordinates(
                if (jitpack) providers.gradleProperty("JITPACK_GROUP").get() else providers.gradleProperty("GROUP").get(),
                if (jitpack) rootProject.name else artifactId,
                if (jitpack) "$name~v$version-jitpack" else version ?: "0.0.0-SNAPSHOT",
            )
            configure(AndroidSingleVariantLibrary(variant = "release", javadocJar = JavadocJar.Empty(), sourcesJar = SourcesJar.Sources()))
            if (centralVersion != null) {
                publishToMavenCentral()
                if (providers.gradleProperty("signingInMemoryKey").isPresent) signAllPublications()
            }
        }
        tasks.withType<com.vanniktech.maven.publish.tasks.JavadocJar>().configureEach {
            from(rootProject.file("LICENSE"), rootProject.file("docs/releases.md"))
            from(rootProject.file("docs/agents")) { into("agents") }
        }
        tasks.withType<Jar>().configureEach {
            isPreserveFileTimestamps = false
            isReproducibleFileOrder = true
        }
        extensions.configure<PublishingExtension> {
            repositories.maven {
                name = "verification"
                url = rootProject.layout.buildDirectory.dir("verification-repository").get().asFile.toURI()
            }
        }

        if (centralVersion != null) {
            val credentials = listOf("mavenCentralUsername", "mavenCentralPassword", "signingInMemoryKey")
                .associateWith(providers::gradleProperty)
            val guard = tasks.register<Exec>("verifyPublicationReservation") {
                workingDir(rootDir)
                commandLine("android-repo-setup", "module_release", "guard", "--module", moduleName, "--version", centralVersion)
                doFirst {
                    credentials.forEach { (key, value) ->
                        require(!value.orNull.isNullOrBlank()) { "Missing release credential: $key" }
                    }
                }
            }
            tasks.matching { it.name.contains("MavenCentral", ignoreCase = true) }.configureEach {
                dependsOn(guard)
            }
        }

        if (nativeJitPack != null && selected) {
            nativeJitPack.verifyCheckout(
                git("HEAD"),
                git("refs/tags/${nativeJitPack.tag}^{commit}"),
                providers.environmentVariable("GIT_COMMIT").orNull,
            )
            tasks.register("publishJitPack") {
                group = "publishing"
                description = "Publish the canonical JitPack release to Maven Local."
                dependsOn("publishToMavenLocal")
            }
        }
    }

    private fun Project.git(ref: String): String = providers.exec {
        workingDir(rootDir)
        commandLine("git", "rev-parse", "--verify", ref)
    }.standardOutput.asText.get().trim()
}
