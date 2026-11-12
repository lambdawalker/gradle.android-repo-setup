package com.apexfission.release

import java.util.Properties
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ModuleIdentityTest {
    @Test fun `nested paths produce stable distinct IDs`() {
        assertEquals("root", defaultModuleId(":"))
        assertEquals("libs--core", defaultModuleId(":libs:core"))
        assertEquals("tools--core", defaultModuleId(":tools:core"))
    }

    private fun inventory() = ModuleInventory(Properties().apply {
        setProperty("public-api.path", ":libs:core")
        setProperty("public-api.directory", "custom/core")
        setProperty("public-api.selected", "true")
        setProperty("public-api.kind", "library")
        setProperty("internal.path", ":internal")
        setProperty("internal.selected", "false")
        setProperty("internal.kind", "library")
        setProperty("demo.path", ":demo")
        setProperty("demo.selected", "true")
        setProperty("demo.kind", "application")
    })

    @Test fun `configured ID follows project path independently of directory`() {
        assertEquals("public-api", inventory().idFor(":libs:core"))
        assertTrue(inventory().selectedLibrary("public-api"))
        assertFalse(inventory().selectedLibrary("libs--core"))
        assertFalse(inventory().selectedLibrary("internal"))
        assertFalse(inventory().selectedLibrary("demo"))
    }

    @Test(expected = IllegalArgumentException::class)
    fun `unselected module cannot be requested`() { inventory().validateSelection("internal") }

    @Test(expected = IllegalArgumentException::class)
    fun `unknown module cannot be requested`() { inventory().validateSelection("unknown") }

    @Test(expected = IllegalArgumentException::class)
    fun `application cannot be requested`() { inventory().validateSelection("demo") }

    @Test fun `old repositories retain path defaults`() {
        val inventory = ModuleInventory(null)
        assertEquals("library", inventory.idFor(":library"))
        assertTrue(inventory.selectedLibrary("library"))
        inventory.validateSelection("library")
    }

    @Test(expected = IllegalArgumentException::class)
    fun `siblings cannot publish`() { requirePublicationSelection(false, "1.0.0") }

    @Test(expected = IllegalArgumentException::class)
    fun `publication without release version fails closed`() { requirePublicationSelection(true, null) }

    @Test fun `explicit version and selection permit publication`() {
        requirePublicationSelection(true, "1.0.0")
    }

    @Test(expected = IllegalArgumentException::class)
    fun `ambiguous configured paths are rejected`() {
        ModuleInventory(Properties().apply {
            setProperty("one.path", ":core")
            setProperty("two.path", ":core")
        })
    }

    @Test fun `ordinary and verification graphs need no release identity`() {
        validatePublicationGraph(emptyList(), false, null, false, "central")
        validatePublicationGraph(listOf(PublicationTarget.VERIFICATION), false, null, false, "central")
    }

    @Test(expected = IllegalArgumentException::class)
    fun `unselected sibling publication invalidates whole graph`() {
        validatePublicationGraph(listOf(PublicationTarget.VERIFICATION, PublicationTarget.LOCAL), false, "1.0.0", false, "central")
    }

    @Test(expected = IllegalArgumentException::class)
    fun `Central graph without version is rejected before reservations`() {
        validatePublicationGraph(listOf(PublicationTarget.CENTRAL), true, null, false, "central")
    }

    @Test(expected = IllegalArgumentException::class)
    fun `JitPack and Central tasks cannot coexist in graph`() {
        validatePublicationGraph(listOf(PublicationTarget.LOCAL, PublicationTarget.CENTRAL), true, "1.0.0", true, "jitpack")
    }

    @Test(expected = IllegalArgumentException::class)
    fun `JitPack cannot use a remote Maven repository`() {
        validatePublicationGraph(listOf(PublicationTarget.REMOTE), true, "1.0.0", true, "jitpack")
    }

    @Test fun `scoped destination graphs are accepted`() {
        validatePublicationGraph(listOf(PublicationTarget.LOCAL), true, "1.0.0", true, "jitpack")
        validatePublicationGraph(listOf(PublicationTarget.REMOTE, PublicationTarget.CENTRAL), true, "1.0.0", false, "central")
    }

    @Test fun `JitPack artifact follows GitHub repository independent of Gradle root name`() {
        val inventory = ModuleInventory(Properties().apply { setProperty("repository.name", "android-library") })
        assertEquals("android-library", inventory.repositoryName("unrelated-gradle-root"))
        assertEquals("legacy-root", ModuleInventory(null).repositoryName("legacy-root"))
        assertEquals("legacy-root", ModuleInventory(Properties()).repositoryName("legacy-root"))
    }
}
