/*
 * Mitts Build for JetBrains IDEs — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

import org.jetbrains.intellij.platform.gradle.IntelliJPlatformType
import org.jetbrains.intellij.platform.gradle.TestFrameworkType

plugins {
    id("org.jetbrains.kotlin.jvm")
    id("org.jetbrains.intellij.platform")
}

group = "codes.prod.mitts"
version = "0.1.0"

kotlin {
    jvmToolchain(25)
}

dependencies {
    testImplementation("junit:junit:4.13.2")
    intellijPlatform {
        intellijIdea("2026.2.5")
        testFramework(TestFrameworkType.Platform)
    }
}

intellijPlatform {
    pluginConfiguration {
        version = project.version.toString()
        ideaVersion {
            sinceBuild = "262"
            untilBuild = provider { null }
        }
    }
    // Binary compatibility with the IDEs it is for, by JetBrains' Plugin Verifier.
    pluginVerification {
        ides {
            create(IntelliJPlatformType.RustRover, "2026.3.1.1")
            create(IntelliJPlatformType.IntellijIdea, "2026.2.5")
        }
    }
}

tasks.test {
    // The end-to-end test runs the real Mitts when MITTS_JB_E2E=1.
    environment("MITTS_JB_E2E", System.getenv("MITTS_JB_E2E") ?: "0")
    testLogging {
        events("passed", "skipped", "failed")
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
    }
}
