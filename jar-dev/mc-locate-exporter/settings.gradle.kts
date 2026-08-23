pluginManagement {
    repositories {
        mavenCentral()
        gradlePluginPortal()
        maven("https://maven.fabricmc.net/")
        maven("https://maven.kikugie.dev/releases") { name = "KikuGie Releases" }
        maven("https://maven.kikugie.dev/snapshots") { name = "KikuGie Snapshots" }
    }
}

plugins {
    id("dev.kikugie.stonecutter") version "0.9.7"
    // Bridges the new 26.1+ Loom variant with the classic one, so one codebase
    // spans both the obfuscated (<=1.21.x) and unobfuscated (26.x) eras.
    id("dev.kikugie.loom-back-compat") version "0.4.2"
    // Lets Gradle auto-provision the JDK each version needs (21 for 1.21.x,
    // 25 for 26.x) instead of requiring them all to be installed.
    id("org.gradle.toolchains.foojay-resolver-convention") version "1.0.0"
}

stonecutter {
    create(rootProject) {
        versions(
            "1.21", "1.21.1", "1.21.2", "1.21.3", "1.21.4", "1.21.5",
            "1.21.6", "1.21.7", "1.21.8", "1.21.9", "1.21.10", "1.21.11",
        )
        version("26.1.x", "26.1.2")
        version("26.2.x", "26.2")
        vcsVersion = "26.2.x"
    }
}

rootProject.name = "mc-locate-exporter"
