import org.gradle.plugin.compatibility.compatibility

plugins {
    `kotlin-dsl`
    id("com.gradle.plugin-publish") version "2.2.1"
}

group = "com.apexfission"
version = "0.1.12"

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

kotlin {
    compilerOptions.jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
}

repositories {
    mavenCentral()
    google()
    gradlePluginPortal()
}

dependencies {
    implementation("com.vanniktech:gradle-maven-publish-plugin:0.37.0")
    testImplementation("junit:junit:4.13.2")
}

gradlePlugin {
    website = "https://github.com/lambdawalker/gradle.android-repo-setup"
    vcsUrl = "https://github.com/lambdawalker/gradle.android-repo-setup.git"

    plugins {
        register("androidRepoSetupRoot") {
            id = "com.apexfission.android-repo-setup.root"
            implementationClass = "com.apexfission.release.AndroidRepoSetupRootPlugin"
            displayName = "Android Repository Setup Root Conventions"
            description = "Adds release documentation tasks backed by the Android Repository Setup CLI."
            tags.set(listOf("android", "documentation", "release"))
            compatibility {
                features {
                    configurationCache.set(false)
                    isolatedProjects.set(false)
                }
            }
        }
        register("androidRepoSetup") {
            id = "com.apexfission.android-repo-setup"
            implementationClass = "com.apexfission.release.AndroidRepoSetupPlugin"
            displayName = "Android Repository Setup Publishing Conventions"
            description = "Configures Android library publications, local verification, and Maven Central and JitPack release tasks."
            tags.set(listOf("android", "maven-central", "jitpack", "publishing"))
            compatibility {
                features {
                    configurationCache.set(false)
                    isolatedProjects.set(false)
                }
            }
        }
    }
}
