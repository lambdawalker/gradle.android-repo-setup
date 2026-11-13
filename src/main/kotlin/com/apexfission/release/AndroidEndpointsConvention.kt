package com.apexfission.release

import com.android.build.api.dsl.ApplicationExtension
import java.net.URI
import java.util.Properties
import org.gradle.api.Project

enum class EndpointValidation {
    NONE,
    HTTPS_URL,
    HOSTNAME,
    ENVIRONMENT,
}

internal enum class EndpointType { STRING, BOOLEAN }

internal data class EndpointBinding(
    val propertyName: String,
    val type: EndpointType,
    val buildConfigName: String?,
    val manifestPlaceholder: String?,
    val defaultValue: String?,
    val requiredInConfig: Boolean,
    val validation: EndpointValidation,
    val allowBlank: Boolean,
)

open class AndroidEndpointsConvention {
    /** Gradle property that points to the optional exported .properties file. */
    var configFileProperty: String = "androidConfigFile"
    private val bindings = mutableListOf<EndpointBinding>()

    fun string(
        property: String,
        buildConfig: String? = null,
        manifestPlaceholder: String? = null,
        defaultValue: String? = null,
        requiredInConfig: Boolean = false,
        validation: EndpointValidation = EndpointValidation.NONE,
        allowBlank: Boolean = true,
    ) {
        add(
            EndpointBinding(
                property,
                EndpointType.STRING,
                buildConfig,
                manifestPlaceholder,
                defaultValue,
                requiredInConfig,
                validation,
                allowBlank,
            ),
        )
    }

    fun boolean(
        property: String,
        buildConfig: String? = null,
        manifestPlaceholder: String? = null,
        defaultValue: Boolean = false,
        requiredInConfig: Boolean = false,
    ) {
        add(
            EndpointBinding(
                property,
                EndpointType.BOOLEAN,
                buildConfig,
                manifestPlaceholder,
                defaultValue.toString(),
                requiredInConfig,
                EndpointValidation.NONE,
                false,
            ),
        )
    }

    private fun add(binding: EndpointBinding) {
        require(binding.propertyName.matches(Regex("[A-Za-z][A-Za-z0-9_.-]*"))) {
            "Invalid Android endpoint property name: ${binding.propertyName}"
        }
        require(bindings.none { it.propertyName == binding.propertyName }) {
            "Android endpoint property already configured: ${binding.propertyName}"
        }
        require(binding.buildConfigName != null || binding.manifestPlaceholder != null) {
            "Endpoint ${binding.propertyName} must bind to BuildConfig, the manifest, or both"
        }
        bindings += binding
    }

    internal fun configuredBindings(): List<EndpointBinding> = bindings.toList()
}

private class EndpointResolver(
    private val project: Project,
    private val bindings: List<EndpointBinding>,
    private val fileProperties: Properties,
) {
    private val byName = bindings.associateBy { it.propertyName }
    private val resolved = mutableMapOf<String, String>()

    fun raw(name: String): String? =
        project.providers.gradleProperty(name).orNull ?: fileProperties.getProperty(name)

    fun resolve(binding: EndpointBinding): String = resolveName(binding.propertyName, linkedSetOf())

    private fun resolveName(name: String, stack: LinkedHashSet<String>): String {
        resolved[name]?.let { return it }
        require(stack.add(name)) {
            "Circular Android endpoint default: ${stack.joinToString(" -> ")} -> $name"
        }
        val binding = byName[name]
        val value = raw(name)
            ?: binding?.defaultValue?.let { expand(it, stack) }
            ?: ""
        stack.remove(name)
        resolved[name] = value
        return value
    }

    private fun expand(template: String, stack: LinkedHashSet<String>): String =
        Regex("\\{([A-Za-z][A-Za-z0-9_.-]*)}").replace(template) { match ->
            resolveName(match.groupValues[1], stack)
        }
}

internal fun Project.configureEndpointConventions(
    android: ApplicationExtension,
    convention: AndroidEndpointsConvention,
) {
    val bindings = convention.configuredBindings()
    if (bindings.isEmpty()) return

    val environmentFile = providers.gradleProperty(convention.configFileProperty).orNull
    val fileProperties = Properties().apply {
        environmentFile?.let { path ->
            val config = rootProject.file(path)
            require(config.isFile) { "Android configuration file not found: $path" }
            config.reader(Charsets.UTF_8).use { load(it) }
        }
    }
    val resolver = EndpointResolver(this, bindings, fileProperties)

    if (environmentFile != null) {
        bindings.filter { it.requiredInConfig }.forEach { binding ->
            require(!resolver.raw(binding.propertyName).isNullOrBlank()) {
                "Missing ${binding.propertyName} in Android environment configuration"
            }
        }
    }

    if (bindings.any { it.buildConfigName != null }) {
        android.buildFeatures.buildConfig = true
    }
    bindings.forEach { binding ->
        val value = resolver.resolve(binding)
        validateEndpoint(binding, value)
        binding.manifestPlaceholder?.let { android.defaultConfig.manifestPlaceholders[it] = value }
        binding.buildConfigName?.let { name ->
            when (binding.type) {
                EndpointType.STRING -> android.defaultConfig.buildConfigField("String", name, quoted(value))
                EndpointType.BOOLEAN -> {
                    val parsed = value.toBooleanStrictOrNull()
                        ?: error("${binding.propertyName} must be true or false")
                    android.defaultConfig.buildConfigField("boolean", name, parsed.toString())
                }
            }
        }
    }
}

private fun validateEndpoint(binding: EndpointBinding, value: String) {
    if (value.isBlank()) {
        require(binding.allowBlank) { "${binding.propertyName} must not be blank" }
        return
    }
    when (binding.validation) {
        EndpointValidation.NONE -> Unit
        EndpointValidation.HOSTNAME -> require(value.matches(Regex("[A-Za-z0-9.-]+"))) {
            "${binding.propertyName} must be a hostname"
        }
        EndpointValidation.ENVIRONMENT -> require(value.matches(Regex("[a-z][a-z0-9-]{0,31}"))) {
            "Invalid ${binding.propertyName}"
        }
        EndpointValidation.HTTPS_URL -> require(runCatching {
            val uri = URI(value)
            uri.scheme == "https" && !uri.host.isNullOrBlank() &&
                uri.userInfo == null && uri.query == null && uri.fragment == null
        }.getOrDefault(false)) {
            "${binding.propertyName} must be an HTTPS URL"
        }
    }
}

private fun quoted(value: String): String {
    require(value.none { it.isISOControl() }) {
        "Android endpoint properties must not contain control characters"
    }
    return "\"" + value.replace("\\", "\\\\").replace("\"", "\\\"") + "\""
}
