# Android application conventions

The `com.apexfission.android-repo-setup.application` plugin centralizes reusable
configuration for runnable Android applications. The consuming build still owns
its application-specific property names and values.

The plugin applies `com.android.application` and exposes an
`androidRepoSetup` extension with two independent convention groups:

- `signing`: optional local/process signing inputs and signed build types.
- `endpoints`: public runtime properties bound to `BuildConfig` fields and/or
  Android manifest placeholders.

The encrypted setup vault remains owned by the `android-repo-setup` CLI. The
Gradle plugin never locates, opens, or decrypts `setup-*.vault` files. A CLI
command or another trusted launcher can expose the selected vault values to the
Gradle child process without coupling the build to the vault format.

## Signing

By default the plugin looks for these values in the process environment and then
in the repository-root ignored `local.properties` file:

```text
ANDROID_KEYSTORE_PATH
ANDROID_KEYSTORE_PASSWORD
ANDROID_KEY_ALIAS
ANDROID_KEY_PASSWORD
```

No signing configuration is installed when the path is absent or blank. When a
path is present, the plugin validates the keystore, certificate alias and private
key before configuring Android signing.

Signing problems are intentionally non-blocking. A missing file, missing or
incorrect password, unreadable keystore, missing alias/certificate, wrong key
password, or configuration-cache conflict leaves registered signing disabled and
project sync continues. Optional signed variants such as `signedDebug` are
created only when the complete signing identity is usable. Normal debug/release
variants keep their existing/default signing state.

The plugin logs a focused warning that names the failing input and, where useful,
whether its value came from an environment variable, `local.properties`, or a
plugin default. Environment variables take precedence over `local.properties`.
Signed local builds require `--no-configuration-cache`.

Projects can preserve older input names or other conventions:

```kotlin
plugins {
    id("com.apexfission.android-repo-setup.application")
}

androidRepoSetup {
    signing {
        keystorePathInput = "MY_KEYSTORE_PATH"
        keystorePasswordInput = "MY_KEYSTORE_PASSWORD"
        keyAliasInput = "MY_KEY_ALIAS"
        keyPasswordInput = "MY_KEY_PASSWORD"
        defaultAlias = "release"
        signDebug = true
        signRelease = true
        createSignedDebug = true
        signedDebugName = "signedDebug"
        debugSourceDirectory = "src/debug/java"
    }
}
```

The optional extra debug type inherits the normal debug build type and dependency
configurations. `debugSourceDirectory` can reuse project-specific debug runtime
implementations without placing that policy in the shared plugin.

## Runtime and endpoint properties

Endpoint bindings read values with this precedence:

1. explicit Gradle property, such as `-PapiBaseUrl=...`;
2. an optional exported Java properties file;
3. the binding's declared default.

Configure the Gradle property that selects the exported file and declare only the
public values the application needs:

```kotlin
import com.apexfission.release.EndpointValidation

androidRepoSetup {
    endpoints {
        configFileProperty = "appConfigFile"

        string(
            property = "environment",
            buildConfig = "APP_ENVIRONMENT",
            defaultValue = "dev",
            validation = EndpointValidation.ENVIRONMENT,
            allowBlank = false,
        )
        string(
            property = "apiBaseUrl",
            buildConfig = "API_BASE_URL",
            requiredInConfig = true,
            validation = EndpointValidation.HTTPS_URL,
        )
        string(
            property = "linkHost",
            buildConfig = "LINK_HOST",
            manifestPlaceholder = "linkHost",
            defaultValue = "{environment}.example.com",
            validation = EndpointValidation.HOSTNAME,
            allowBlank = false,
        )
        boolean(
            property = "captureEnabled",
            buildConfig = "CAPTURE_ENABLED",
            defaultValue = false,
        )
    }
}
```

Defaults may reference another configured property using `{propertyName}`.
Circular defaults are rejected. Available string validators are `NONE`,
`HTTPS_URL`, `HOSTNAME`, and `ENVIRONMENT`. Boolean bindings accept only
`true` or `false`.

When an exported configuration file is selected, bindings marked
`requiredInConfig = true` must be supplied by that file or an explicit Gradle
property. This lets generated deployment handoffs fail closed while retaining
developer defaults when no handoff is selected.

These endpoint values are compiled into the application and therefore must be
public configuration. Do not bind passwords, tokens, private keys, or other
credentials into `BuildConfig` or the manifest.
