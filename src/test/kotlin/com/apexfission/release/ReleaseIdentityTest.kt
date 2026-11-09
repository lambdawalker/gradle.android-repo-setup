package com.apexfission.release

import org.gradle.api.tasks.Exec
import org.gradle.testfixtures.ProjectBuilder
import org.junit.Assert.assertEquals
import org.junit.Test

class ReleaseIdentityTest {
    @Test fun rootPluginRegistersExistingDocumentationCommands() {
        val project = ProjectBuilder.builder().build()
        project.pluginManager.apply("com.apexfission.android-repo-setup.root")
        val verify = project.tasks.getByName("verifyImportDocs") as Exec
        assertEquals(listOf("android-repo-setup", "module_release", "verify"), verify.commandLine)
    }

    @Test fun parsesBothJitPackTagRepresentations() {
        for (tag in listOf("demo-common/v0.1.2-jitpack", "demo-common~v0.1.2-jitpack")) {
            assertEquals(JitPackRelease("demo-common", "0.1.2"), JitPackRelease.parse(tag))
        }
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejectsAnUnscopedTag() {
        JitPackRelease.parse("v0.1.2")
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejectsTagsThatDoNotIdentifyTheCheckout() {
        JitPackRelease("demo-common", "0.1.2").verifyCheckout("head", "other", "head")
    }
}
