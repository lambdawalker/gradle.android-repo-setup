package com.apexfission.release

import com.android.build.api.dsl.ApplicationExtension
import java.security.KeyStore
import java.security.PrivateKey
import java.util.Properties
import org.gradle.api.GradleException
import org.gradle.api.Project

open class AndroidSigningConvention {
    /** Environment/local.properties key containing the keystore path. */
    var keystorePathInput: String = "ANDROID_KEYSTORE_PATH"
    var keystorePasswordInput: String = "ANDROID_KEYSTORE_PASSWORD"
    var keyAliasInput: String = "ANDROID_KEY_ALIAS"
    var keyPasswordInput: String = "ANDROID_KEY_PASSWORD"
    var defaultAlias: String = "android"
    var signDebug: Boolean = true
    var signRelease: Boolean = true
    var createSignedDebug: Boolean = false
    var signedDebugName: String = "signedDebug"
    var debugSourceDirectory: String? = null
}

private fun Project.signingProperties(): Properties = Properties().apply {
    val contents = providers.fileContents(rootProject.layout.projectDirectory.file("local.properties")).asText.orNull
    contents?.reader()?.use(::load)
}

private fun Project.signingSetting(properties: Properties, name: String): String? =
    providers.environmentVariable(name).orNull ?: properties.getProperty(name)

internal fun Project.configureSigningConventions(
    android: ApplicationExtension,
    convention: AndroidSigningConvention,
) {
    val local = signingProperties()
    val keystorePath = signingSetting(local, convention.keystorePathInput)?.takeIf { it.isNotBlank() }
        ?: return

    require(!gradle.startParameter.isConfigurationCacheRequested) {
        "Registered Android signing requires --no-configuration-cache to avoid persisting signing credentials"
    }
    val keystore = rootProject.file(keystorePath)
    require(keystore.isFile) {
        "${convention.keystorePathInput} must reference an existing keystore"
    }
    val storePassword = signingSetting(local, convention.keystorePasswordInput)
    require(!storePassword.isNullOrBlank()) {
        "Set ${convention.keystorePasswordInput} when ${convention.keystorePathInput} is set"
    }
    val alias = signingSetting(local, convention.keyAliasInput)
        ?.takeIf { it.isNotBlank() } ?: convention.defaultAlias
    val keyPassword = signingSetting(local, convention.keyPasswordInput)
        ?.takeIf { it.isNotBlank() } ?: storePassword

    validateSigningKey(
        keystore = keystore,
        alias = alias,
        storePassword = storePassword,
        keyPassword = keyPassword,
        storePasswordInput = convention.keystorePasswordInput,
        aliasInput = convention.keyAliasInput,
        keyPasswordInput = convention.keyPasswordInput,
    )

    val registered = android.signingConfigs.findByName("registered")
        ?: android.signingConfigs.create("registered")
    registered.storeFile = keystore
    registered.storePassword = storePassword
    registered.keyAlias = alias
    registered.keyPassword = keyPassword

    if (convention.signDebug) {
        android.buildTypes.named("debug") { signingConfig = registered }
    }
    if (convention.signRelease) {
        android.buildTypes.named("release") { signingConfig = registered }
    }
    if (convention.createSignedDebug) {
        require(android.buildTypes.findByName(convention.signedDebugName) == null) {
            "Android build type ${convention.signedDebugName} already exists"
        }
        android.buildTypes.create(convention.signedDebugName) {
            initWith(android.buildTypes.getByName("debug"))
            signingConfig = registered
            matchingFallbacks += "debug"
        }
        convention.debugSourceDirectory?.takeIf { it.isNotBlank() }?.let { source ->
            android.sourceSets.maybeCreate(convention.signedDebugName).java.srcDir(source)
        }
        listOf("Implementation", "CompileOnly", "RuntimeOnly", "AnnotationProcessor").forEach { scope ->
            configurations.matching { it.name == "${convention.signedDebugName}$scope" }.configureEach {
                extendsFrom(configurations.getByName("debug$scope"))
            }
        }
    }
}

private fun validateSigningKey(
    keystore: java.io.File,
    alias: String,
    storePassword: String,
    keyPassword: String,
    storePasswordInput: String,
    aliasInput: String,
    keyPasswordInput: String,
) {
    val storeChars = storePassword.toCharArray()
    val keyChars = keyPassword.toCharArray()
    try {
        val store = try {
            KeyStore.getInstance(keystore, storeChars)
        } catch (_: Exception) {
            throw GradleException("Cannot open signing keystore; check $storePasswordInput and the keystore format")
        }
        require(store.isKeyEntry(alias) && store.getCertificate(alias) != null) {
            "$aliasInput must identify a private signing key with a certificate"
        }
        val key = try {
            store.getKey(alias, keyChars)
        } catch (_: Exception) {
            throw GradleException("Cannot unlock signing key; check $keyPasswordInput (defaults to the store password)")
        }
        require(key is PrivateKey) { "$aliasInput must identify a private signing key" }
    } finally {
        storeChars.fill('\u0000')
        keyChars.fill('\u0000')
    }
}
