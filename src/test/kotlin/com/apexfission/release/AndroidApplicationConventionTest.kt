package com.apexfission.release

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
}
