package com.apexfission.release

import com.android.build.api.dsl.ApplicationExtension
import java.io.File
import java.security.KeyStore
import java.security.MessageDigest
import java.security.PrivateKey
import java.security.cert.X509Certificate
import java.security.UnrecoverableKeyException
import java.util.Properties
import org.gradle.api.Project
import org.gradle.kotlin.dsl.configureEach
import org.gradle.kotlin.dsl.create
import org.gradle.kotlin.dsl.named

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

internal enum class SigningValueSource(val description: String) {
    ENVIRONMENT("environment variable"),
    LOCAL_PROPERTIES("local.properties"),
    DEFAULT("plugin default"),
}

internal data class SigningSetting(
    val value: String,
    val source: SigningValueSource,
)

internal data class SigningIssue(
    val summary: String,
    val guidance: String? = null,
)

private fun Project.signingProperties(): Properties = Properties().apply {
    val contents = providers.fileContents(
        rootProject.layout.projectDirectory.file("local.properties"),
    ).asText.orNull
    contents?.reader()?.use { load(it) }
}

private fun Project.signingSetting(properties: Properties, name: String): SigningSetting? {
    providers.environmentVariable(name).orNull?.let {
        return SigningSetting(it, SigningValueSource.ENVIRONMENT)
    }
    properties.getProperty(name)?.let {
        return SigningSetting(it, SigningValueSource.LOCAL_PROPERTIES)
    }
    return null
}

private fun Project.reportSigningUnavailable(
    convention: AndroidSigningConvention,
    issue: SigningIssue,
) {
    val consequences = buildList {
        if (convention.createSignedDebug) {
            add("The '${convention.signedDebugName}' build variant will not be created.")
        }
        if (convention.signDebug) {
            add("The normal debug variant keeps its default Android debug signing.")
        }
        if (convention.signRelease) {
            add("The normal release variant keeps its existing signing state.")
        }
        add("Project sync continues normally.")
    }.joinToString(" ")

    val guidance = issue.guidance?.let { " $it" }.orEmpty()
    logger.warn(
        "[android-repo-setup] Local Android signing unavailable: ${issue.summary}. " +
            consequences + guidance,
    )
}

private fun Throwable.causes(): Sequence<Throwable> =
    generateSequence(this) { it.cause }.take(8)

private fun Throwable.looksLikePasswordFailure(): Boolean =
    causes().any { error ->
        error is UnrecoverableKeyException || error.message.orEmpty().lowercase().let { message ->
            "password" in message &&
                ("incorrect" in message ||
                    "verification failed" in message ||
                    "failed to decrypt" in message ||
                    "tampered" in message)
        }
    }

private fun Throwable.looksLikeFormatFailure(): Boolean =
    causes().any { error ->
        error.message.orEmpty().lowercase().let { message ->
            "unrecognized keystore format" in message ||
                "invalid keystore format" in message ||
                "not a jks" in message ||
                "not a pkcs12" in message
        }
    }

private fun aliases(store: KeyStore): String {
    val values = store.aliases().toList().sorted()
    return when {
        values.isEmpty() -> "The keystore contains no aliases."
        values.size <= 8 -> "Available aliases: ${values.joinToString(", ")}."
        else -> "Available aliases include: ${values.take(8).joinToString(", ")} (+${values.size - 8} more)."
    }
}

internal fun inspectSigningKey(
    keystore: File,
    alias: SigningSetting,
    storePassword: SigningSetting,
    keyPassword: SigningSetting,
    storePasswordInput: String,
    aliasInput: String,
    keyPasswordInput: String,
): SigningIssue? {
    val storeChars = storePassword.value.toCharArray()
    val keyChars = keyPassword.value.toCharArray()
    try {
        val store = try {
            KeyStore.getInstance(keystore, storeChars)
        } catch (error: Exception) {
            return when {
                error.looksLikePasswordFailure() -> SigningIssue(
                    "$storePasswordInput from ${storePassword.source.description} did not unlock " +
                        "keystore '${keystore.path}'",
                    "Check that password source first. Environment variables override local.properties.",
                )
                error.looksLikeFormatFailure() -> SigningIssue(
                    "Keystore '${keystore.path}' is not a readable JKS or PKCS12 keystore",
                    "Verify that $aliasInput points to the intended Android signing keystore and that the file is not corrupt.",
                )
                else -> SigningIssue(
                    "Keystore '${keystore.path}' could not be opened with $storePasswordInput " +
                        "from ${storePassword.source.description}",
                    "The file exists, so verify the store password and keystore format. " +
                        "Environment variables override local.properties.",
                )
            }
        }

        if (!store.containsAlias(alias.value)) {
            return SigningIssue(
                "Alias '${alias.value}' from ${alias.source.description} was not found in " +
                    "keystore '${keystore.path}'",
                "${aliases(store)} Check $aliasInput.",
            )
        }
        if (!store.isKeyEntry(alias.value)) {
            return SigningIssue(
                "Alias '${alias.value}' exists in keystore '${keystore.path}' but is not a private-key entry",
                "Choose the alias that owns the Android signing private key.",
            )
        }
        if (store.getCertificate(alias.value) == null) {
            return SigningIssue(
                "Alias '${alias.value}' in keystore '${keystore.path}' has no signing certificate",
                "Use a private-key entry that includes its certificate chain.",
            )
        }

        val key = try {
            store.getKey(alias.value, keyChars)
        } catch (_: Exception) {
            return SigningIssue(
                "$keyPasswordInput from ${keyPassword.source.description} did not unlock private " +
                    "key alias '${alias.value}' in '${keystore.path}'",
                if (keyPassword.source == SigningValueSource.DEFAULT) {
                    "$keyPasswordInput was omitted, so the store password was tried. Set $keyPasswordInput when the key uses a different password."
                } else {
                    "Check $keyPasswordInput. Environment variables override local.properties."
                },
            )
        }
        if (key !is PrivateKey) {
            return SigningIssue(
                "Alias '${alias.value}' in keystore '${keystore.path}' does not contain a private signing key",
                "Choose the Android application signing key alias.",
            )
        }
        return null
    } finally {
        storeChars.fill('\u0000')
        keyChars.fill('\u0000')
    }
}

private fun certificateFingerprint(certificate: java.security.cert.Certificate): String =
    MessageDigest.getInstance("SHA-256")
        .digest(certificate.encoded)
        .joinToString(":") { byte -> "%02X".format(byte.toInt() and 0xff) }

internal fun Project.verifyLocalSigning(convention: AndroidSigningConvention) {
    val local = signingProperties()
    val keystorePath = signingSetting(local, convention.keystorePathInput)
        ?.takeIf { it.value.isNotBlank() }
        ?: throw org.gradle.api.GradleException(
            "${convention.keystorePathInput} is not configured. Run the setup Local Gradle signing step first."
        )

    val keystore = rootProject.file(keystorePath.value)
    if (!keystore.isFile) {
        throw org.gradle.api.GradleException(
            "${convention.keystorePathInput} from ${keystorePath.source.description} points to missing file '${keystore.path}'"
        )
    }

    val storePassword = signingSetting(local, convention.keystorePasswordInput)
        ?.takeIf { it.value.isNotBlank() }
        ?: throw org.gradle.api.GradleException(
            "${convention.keystorePasswordInput} is missing while ${convention.keystorePathInput} points to '${keystore.path}'"
        )
    val alias = signingSetting(local, convention.keyAliasInput)
        ?.takeIf { it.value.isNotBlank() }
        ?: SigningSetting(convention.defaultAlias, SigningValueSource.DEFAULT)
    val keyPassword = signingSetting(local, convention.keyPasswordInput)
        ?.takeIf { it.value.isNotBlank() }
        ?: SigningSetting(storePassword.value, SigningValueSource.DEFAULT)

    inspectSigningKey(
        keystore = keystore,
        alias = alias,
        storePassword = storePassword,
        keyPassword = keyPassword,
        storePasswordInput = convention.keystorePasswordInput,
        aliasInput = convention.keyAliasInput,
        keyPasswordInput = convention.keyPasswordInput,
    )?.let { issue ->
        throw org.gradle.api.GradleException(
            buildString {
                append(issue.summary)
                issue.guidance?.let { append(". ").append(it) }
            }
        )
    }

    val storeChars = storePassword.value.toCharArray()
    try {
        val store = KeyStore.getInstance(keystore, storeChars)
        val certificate = store.getCertificate(alias.value)
            ?: throw org.gradle.api.GradleException(
                "Alias '${alias.value}' has no certificate in '${keystore.path}'"
            )
        val fingerprint = certificateFingerprint(certificate)

        logger.lifecycle("Local Android signing verification: PASS")
        logger.lifecycle("  Keystore: ${keystore.absolutePath}")
        logger.lifecycle("  Keystore path source: ${keystorePath.source.description}")
        logger.lifecycle("  Alias: ${alias.value}")
        logger.lifecycle("  Alias source: ${alias.source.description}")
        logger.lifecycle("  SHA-256: $fingerprint")
        if (certificate is X509Certificate) {
            logger.lifecycle("  Subject: ${certificate.subjectX500Principal.name}")
            logger.lifecycle("  Issuer: ${certificate.issuerX500Principal.name}")
            logger.lifecycle("  Valid from: ${certificate.notBefore}")
            logger.lifecycle("  Valid until: ${certificate.notAfter}")
            certificate.checkValidity()
            logger.lifecycle("  Certificate validity: current")
        }
        logger.lifecycle(
            "  Expected signed variant: " +
                if (convention.createSignedDebug) convention.signedDebugName else "(none configured)"
        )
        logger.lifecycle("No signing passwords were printed.")
    } finally {
        storeChars.fill('\u0000')
    }
}

internal fun Project.configureSigningConventions(
    android: ApplicationExtension,
    convention: AndroidSigningConvention,
) {
    val local = signingProperties()
    val keystorePath = signingSetting(local, convention.keystorePathInput)
        ?.takeIf { it.value.isNotBlank() }
        ?: return

    if (gradle.startParameter.isConfigurationCacheRequested) {
        reportSigningUnavailable(
            convention,
            SigningIssue(
                "registered signing is disabled while Gradle configuration cache is enabled",
                "Disable configuration cache for signed local builds with --no-configuration-cache.",
            ),
        )
        return
    }

    val keystore = rootProject.file(keystorePath.value)
    if (!keystore.isFile) {
        reportSigningUnavailable(
            convention,
            SigningIssue(
                "${convention.keystorePathInput} from ${keystorePath.source.description} points to " +
                    "missing file '${keystore.path}'",
                "Run local signing setup again or correct ${convention.keystorePathInput}.",
            ),
        )
        return
    }

    val storePassword = signingSetting(local, convention.keystorePasswordInput)
    if (storePassword == null || storePassword.value.isBlank()) {
        reportSigningUnavailable(
            convention,
            SigningIssue(
                "${convention.keystorePasswordInput} is missing while " +
                    "${convention.keystorePathInput} points to '${keystore.path}'",
                "Set ${convention.keystorePasswordInput} in the environment or ignored local.properties.",
            ),
        )
        return
    }

    val alias = signingSetting(local, convention.keyAliasInput)
        ?.takeIf { it.value.isNotBlank() }
        ?: SigningSetting(convention.defaultAlias, SigningValueSource.DEFAULT)
    val keyPassword = signingSetting(local, convention.keyPasswordInput)
        ?.takeIf { it.value.isNotBlank() }
        ?: SigningSetting(storePassword.value, SigningValueSource.DEFAULT)

    val issue = inspectSigningKey(
        keystore = keystore,
        alias = alias,
        storePassword = storePassword,
        keyPassword = keyPassword,
        storePasswordInput = convention.keystorePasswordInput,
        aliasInput = convention.keyAliasInput,
        keyPasswordInput = convention.keyPasswordInput,
    )
    if (issue != null) {
        reportSigningUnavailable(convention, issue)
        return
    }

    val registered = android.signingConfigs.findByName("registered")
        ?: android.signingConfigs.create("registered")
    registered.storeFile = keystore
    registered.storePassword = storePassword.value
    registered.keyAlias = alias.value
    registered.keyPassword = keyPassword.value

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
