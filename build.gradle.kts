plugins {
    `kotlin-dsl`
    `java-gradle-plugin`
    `maven-publish`
}

group = "com.apexfission"
version = "0.1.0-SNAPSHOT"

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
    plugins {
        register("androidRepoSetupRoot") {
            id = "com.apexfission.android-repo-setup.root"
            implementationClass = "com.apexfission.release.AndroidRepoSetupRootPlugin"
        }
        register("androidRepoSetup") {
            id = "com.apexfission.android-repo-setup"
            implementationClass = "com.apexfission.release.AndroidRepoSetupPlugin"
        }
    }
}
