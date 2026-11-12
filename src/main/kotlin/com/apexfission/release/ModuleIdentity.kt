package com.apexfission.release

import java.util.Properties

internal fun defaultModuleId(path: String): String =
    path.removePrefix(":").replace(":", "--").ifEmpty { "root" }

internal class ModuleInventory(private val properties: Properties?) {
    private val ids = properties?.stringPropertyNames()?.filter { it.endsWith(".path") }
        ?.map { it.removeSuffix(".path") }.orEmpty()

    init {
        require(ids.map { properties!!.getProperty("$it.path") }.distinct().size == ids.size) {
            "Module inventory contains duplicate Gradle project paths"
        }
    }

    fun repositoryName(fallback: String): String =
        properties?.getProperty("repository.name")?.takeIf { it.isNotBlank() } ?: fallback

    fun idFor(path: String): String = ids.singleOrNull { properties!!.getProperty("$it.path") == path }
        ?: defaultModuleId(path)

    fun selectedLibrary(id: String): Boolean = properties == null ||
        id in ids && properties.getProperty("$id.selected") == "true" &&
        properties.getProperty("$id.kind") == "library"

    fun validateSelection(id: String?) {
        require(id == null || selectedLibrary(id)) { "Release module '$id' is not a selected library in modules.properties" }
    }
}

internal fun requirePublicationSelection(selected: Boolean, version: String?) {
    require(selected && version != null) {
        "Publishing requires an explicitly selected library and release version; use the verification repository for local checks"
    }
}

internal enum class PublicationTarget { VERIFICATION, LOCAL, REMOTE, CENTRAL }

internal fun validatePublicationGraph(
    targets: List<PublicationTarget>,
    selected: Boolean,
    version: String?,
    jitpack: Boolean,
    publisher: String,
) {
    targets.filter { it != PublicationTarget.VERIFICATION }.forEach { target ->
        requirePublicationSelection(selected, version)
        require(!jitpack || target == PublicationTarget.LOCAL) {
            "JitPack releases may publish only to Maven Local; Central and JitPack tasks cannot be mixed"
        }
        require(target != PublicationTarget.CENTRAL || publisher == "central") {
            "Maven Central tasks require the Central release destination"
        }
    }
}
