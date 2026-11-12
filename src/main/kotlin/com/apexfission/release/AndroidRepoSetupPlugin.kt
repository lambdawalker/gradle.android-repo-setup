package com.apexfission.release

import com.vanniktech.maven.publish.AndroidSingleVariantLibrary
import com.vanniktech.maven.publish.JavadocJar
import com.vanniktech.maven.publish.MavenPublishBaseExtension
import com.vanniktech.maven.publish.SourcesJar
import java.util.Properties
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.api.publish.PublishingExtension
import org.gradle.api.publish.maven.tasks.AbstractPublishToMaven
import org.gradle.api.publish.maven.tasks.PublishToMavenRepository
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
        require(nativeJitPack == null || requestedModule == null || requestedModule == nativeJitPack.module) {
            "releaseModule conflicts with the native JitPack tag"
        }
        require(nativeJitPack == null || requestedVersion == null || requestedVersion == nativeJitPack.version) {
            "releaseVersion conflicts with the native JitPack tag"
        }
        val inventoryFile = rootProject.layout.projectDirectory.file(".project/setup/modules.properties")
        val inventory = ModuleInventory(providers.fileContents(inventoryFile).asText.orNull?.let {
            Properties().apply { it.reader().use(::load) }
        })
        val moduleName = inventory.idFor(path)
        val selectedModule = nativeJitPack?.module ?: requestedModule
        inventory.validateSelection(selectedModule)
        val selected = selectedModule == moduleName && inventory.selectedLibrary(moduleName)
        val version = (nativeJitPack?.version ?: requestedVersion).takeIf { selected }
        val verificationJitPack = providers.gradleProperty("jitpackBuild").orNull == "true"
        val jitpack = selected && (nativeJitPack != null || verificationJitPack)
        require(!verificationJitPack || requestedVersion != null &&
            providers.gradleProperty("jitpackScopedTag").orNull == "true") {
            "A destination-scoped JitPack release identity is required"
        }
        val destination = providers.environmentVariable("RELEASE_REPOSITORY").getOrElse(
            if (nativeJitPack != null || verificationJitPack) "jitpack" else "maven-central"
        )
        val repositoryConfig = rootProject.layout.projectDirectory.file(".project/setup/repositories.properties")
        val publisher = providers.fileContents(repositoryConfig).asText.orNull?.let { contents ->
            Properties().apply { contents.reader().use(::load) }.getProperty("$destination.publisher")
        } ?: when (destination) {
            "maven-central" -> "central"
            "jitpack" -> "jitpack"
            else -> null
        }
        require(publisher == "central" || publisher == "jitpack") { "Unknown release destination: $destination" }
        require(!(nativeJitPack != null || verificationJitPack) || publisher == "jitpack") {
            "JitPack and Maven Central publishing cannot be mixed"
        }
        require(publisher != "jitpack" || nativeJitPack != null || verificationJitPack) {
            "JitPack publishing requires a destination-scoped JitPack release identity"
        }
        val centralVersion = version.takeIf { !jitpack && publisher == "central" }
        val artifactId = providers.gradleProperty("POM_ARTIFACT_ID.$moduleName")
            .orElse(providers.gradleProperty("POM_ARTIFACT_ID"))
            .getOrElse(moduleName)

        extensions.configure<MavenPublishBaseExtension> {
            coordinates(
                if (jitpack) providers.gradleProperty("JITPACK_GROUP").get() else providers.gradleProperty("GROUP").get(),
                if (jitpack) inventory.repositoryName(rootProject.name) else artifactId,
                if (jitpack) "$moduleName~v$version-jitpack" else version ?: "0.0.0-SNAPSHOT",
            )
            configure(AndroidSingleVariantLibrary(variant = "release", javadocJar = JavadocJar.Empty(), sourcesJar = SourcesJar.Sources()))
            if (centralVersion != null) {
                publishToMavenCentral()
                if (providers.gradleProperty("signingInMemoryKey").isPresent) signAllPublications()
            }
        }
        tasks.withType<com.vanniktech.maven.publish.tasks.JavadocJar>().configureEach {
            from(rootProject.file("LICENSE"), rootProject.file("docs/releases.md"))
            from(rootProject.file("docs/agents")) { include("**/*.md", "**/*.markdown"); into("agents") }
            from(projectDir) { include("README.md", "README.markdown") }
            from(file("docs")) { include("**/*.md", "**/*.markdown"); into("docs") }
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

        // Reject the complete graph before any task (including reservation guards) executes.
        // Each participating module checks its own tasks, so an aggregate publish cannot
        // publish the selected module before discovering an unselected sibling.
        gradle.taskGraph.whenReady {
            val targets = allTasks.filter { it.project == this@configureRelease }.mapNotNull { task ->
                when {
                    task is PublishToMavenRepository -> {
                        val verification = task.repository.name == "verification" &&
                            task.repository.url == rootProject.layout.buildDirectory
                                .dir("verification-repository").get().asFile.toURI()
                        if (verification) PublicationTarget.VERIFICATION else PublicationTarget.REMOTE
                    }
                    task is AbstractPublishToMaven -> PublicationTarget.LOCAL
                    task.name.contains("MavenCentral", ignoreCase = true) -> PublicationTarget.CENTRAL
                    else -> null
                }
            }
            validatePublicationGraph(targets, selected, version, jitpack, publisher)
        }

        tasks.withType<AbstractPublishToMaven>().configureEach {
            doFirst {
                val verification = this is PublishToMavenRepository && repository.name == "verification" &&
                    repository.url == rootProject.layout.buildDirectory.dir("verification-repository").get().asFile.toURI()
                if (!verification) {
                    requirePublicationSelection(selected, version)
                    require(!jitpack || this !is PublishToMavenRepository) {
                        "JitPack releases may publish only to Maven Local"
                    }
                }
            }
        }
        // Central portal tasks are not AbstractPublishToMaven tasks.
        tasks.matching { it.name.contains("MavenCentral", ignoreCase = true) }.configureEach {
            doFirst {
                requirePublicationSelection(selected, version)
                require(!jitpack && publisher == "central") { "JitPack and Maven Central publishing cannot be mixed" }
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
