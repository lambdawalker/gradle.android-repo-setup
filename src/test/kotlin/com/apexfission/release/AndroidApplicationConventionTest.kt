package com.apexfission.release

import java.io.FileOutputStream
import java.nio.file.Files
import java.security.KeyStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidApplicationConventionTest {
    @Test fun `signing defaults are generic and opt in to the extra debug variant`() {
        val signing = AndroidSigningConvention()
        assertEquals("ANDROID_KEYSTORE_PATH", signing.keystorePathInput)
        assertEquals("ANDROID_KEYSTORE_PASSWORD", signing.keystorePasswordInput)
        assertEquals("ANDROID_KEY_ALIAS", signing.keyAliasInput)
        assertEquals("ANDROID_KEY_PASSWORD", signing.keyPasswordInput)
        assertTrue(signing.signDebug)
        assertTrue(signing.signRelease)
        assertFalse(signing.createSignedDebug)
    }

    @Test fun `signing validation identifies a wrong store password without throwing`() {
        val file = Files.createTempFile("signing-", ".jks").toFile()
        try {
            val password = "correct-password".toCharArray()
            KeyStore.getInstance("JKS").apply {
                load(null, password)
                FileOutputStream(file).use { store(it, password) }
            }
            val issue = inspectSigningKey(
                keystore = file,
                alias = SigningSetting("release", SigningValueSource.DEFAULT),
                storePassword = SigningSetting("wrong-password", SigningValueSource.LOCAL_PROPERTIES),
                keyPassword = SigningSetting("wrong-password", SigningValueSource.DEFAULT),
                storePasswordInput = "ANDROID_KEYSTORE_PASSWORD",
                aliasInput = "ANDROID_KEY_ALIAS",
                keyPasswordInput = "ANDROID_KEY_PASSWORD",
            )
            assertNotNull(issue)
            assertTrue(issue!!.summary.contains("ANDROID_KEYSTORE_PASSWORD from local.properties"))
            assertTrue(issue.summary.contains("did not unlock"))
        } finally {
            file.delete()
        }
    }

    @Test fun `signing validation reports the missing alias and available aliases`() {
        val file = Files.createTempFile("signing-", ".jks").toFile()
        try {
            val password = "fixture-password".toCharArray()
            KeyStore.getInstance("JKS").apply {
                load(null, password)
                FileOutputStream(file).use { store(it, password) }
            }
            val issue = inspectSigningKey(
                keystore = file,
                alias = SigningSetting("missing-alias", SigningValueSource.ENVIRONMENT),
                storePassword = SigningSetting("fixture-password", SigningValueSource.ENVIRONMENT),
                keyPassword = SigningSetting("fixture-password", SigningValueSource.DEFAULT),
                storePasswordInput = "ANDROID_KEYSTORE_PASSWORD",
                aliasInput = "ANDROID_KEY_ALIAS",
                keyPasswordInput = "ANDROID_KEY_PASSWORD",
            )
            assertNotNull(issue)
            assertTrue(issue!!.summary.contains("missing-alias"))
            assertTrue(issue.summary.contains("environment variable"))
            assertTrue(issue.guidance.orEmpty().contains("contains no aliases"))
        } finally {
            file.delete()
        }
    }

    @Test fun `signing validation distinguishes an unreadable keystore file`() {
        val file = Files.createTempFile("signing-", ".txt").toFile()
        try {
            file.writeText("not a keystore")
            val issue = inspectSigningKey(
                keystore = file,
                alias = SigningSetting("release", SigningValueSource.DEFAULT),
                storePassword = SigningSetting("fixture-password", SigningValueSource.LOCAL_PROPERTIES),
                keyPassword = SigningSetting("fixture-password", SigningValueSource.DEFAULT),
                storePasswordInput = "ANDROID_KEYSTORE_PASSWORD",
                aliasInput = "ANDROID_KEY_ALIAS",
                keyPasswordInput = "ANDROID_KEY_PASSWORD",
                keystorePathInput = "CUSTOM_KEYSTORE_PATH",
            )
            assertNotNull(issue)
            assertTrue(
                issue!!.summary.contains("not a readable JKS or PKCS12") ||
                    issue.summary.contains("could not be opened"),
            )
            if (issue.summary.contains("not a readable JKS or PKCS12")) {
                assertTrue(issue.guidance.orEmpty().contains("CUSTOM_KEYSTORE_PATH"))
                assertFalse(issue.guidance.orEmpty().contains("ANDROID_KEY_ALIAS"))
            }
        } finally {
            file.delete()
        }
    }

    @Test fun `endpoint templates can reference other configured properties`() {
        val endpoints = AndroidEndpointsConvention().apply {
            string(
                property = "environment",
                buildConfig = "ENVIRONMENT",
                defaultValue = "dev",
                validation = EndpointValidation.ENVIRONMENT,
                allowBlank = false,
            )
            string(
                property = "linkHost",
                buildConfig = "LINK_HOST",
                manifestPlaceholder = "linkHost",
                defaultValue = "{environment}.example.com",
                validation = EndpointValidation.HOSTNAME,
                allowBlank = false,
            )
        }
        val bindings = endpoints.configuredBindings()
        val resolver = EndpointResolver(bindings) { null }
        assertEquals("dev", resolver.resolve(bindings[0]))
        assertEquals("dev.example.com", resolver.resolve(bindings[1]))
        bindings.forEach { validateEndpoint(it, resolver.resolve(it)) }
    }

    @Test fun `explicit endpoint values override defaults`() {
        val endpoints = AndroidEndpointsConvention().apply {
            string("environment", buildConfig = "ENVIRONMENT", defaultValue = "dev")
            string("host", buildConfig = "HOST", defaultValue = "{environment}.example.com")
        }
        val bindings = endpoints.configuredBindings()
        val values = mapOf("environment" to "qa", "host" to "api.example.net")
        val resolver = EndpointResolver(bindings, values::get)
        assertEquals("qa", resolver.resolve(bindings[0]))
        assertEquals("api.example.net", resolver.resolve(bindings[1]))
    }

    @Test(expected = IllegalArgumentException::class)
    fun `duplicate endpoint declarations fail closed`() {
        AndroidEndpointsConvention().apply {
            string("api", buildConfig = "API")
            string("api", manifestPlaceholder = "api")
        }
    }

    @Test(expected = IllegalArgumentException::class)
    fun `circular endpoint defaults are rejected`() {
        val endpoints = AndroidEndpointsConvention().apply {
            string("one", buildConfig = "ONE", defaultValue = "{two}")
            string("two", buildConfig = "TWO", defaultValue = "{one}")
        }
        val bindings = endpoints.configuredBindings()
        EndpointResolver(bindings) { null }.resolve(bindings[0])
    }

    @Test(expected = IllegalArgumentException::class)
    fun `invalid https endpoint is rejected`() {
        val endpoints = AndroidEndpointsConvention().apply {
            string(
                "api",
                buildConfig = "API",
                validation = EndpointValidation.HTTPS_URL,
                allowBlank = false,
            )
        }
        val binding = endpoints.configuredBindings().single()
        validateEndpoint(binding, "http://example.com")
    }

    @Test(expected = IllegalArgumentException::class)
    fun `unknown endpoint template placeholder is rejected`() {
        val endpoints = AndroidEndpointsConvention().apply {
            string("host", buildConfig = "HOST", defaultValue = "{missing}.example.com")
        }
        val binding = endpoints.configuredBindings().single()
        EndpointResolver(endpoints.configuredBindings()) { null }.resolve(binding)
    }

    @Test
    fun `malformed hostname endpoints are rejected`() {
        val endpoints = AndroidEndpointsConvention().apply {
            string(
                "host",
                buildConfig = "HOST",
                validation = EndpointValidation.HOSTNAME,
                allowBlank = false,
            )
        }
        val binding = endpoints.configuredBindings().single()
        validateEndpoint(binding, "api-1.example.com")
        for (invalid in listOf("...", "---", ".example.com", "example..com", "-bad.example.com", "bad-.example.com")) {
            val failed = runCatching { validateEndpoint(binding, invalid) }.isFailure
            assertTrue("Expected invalid hostname to be rejected: $invalid", failed)
        }
    }
}
