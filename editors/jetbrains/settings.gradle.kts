/*
 * Mitts Build for JetBrains IDEs — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

import org.jetbrains.intellij.platform.gradle.extensions.intellijPlatform

rootProject.name = "mitts-jetbrains"

pluginManagement {
    plugins {
        id("org.jetbrains.kotlin.jvm") version "2.4.21"
    }
}

plugins {
    id("org.gradle.toolchains.foojay-resolver-convention") version "1.0.0"
    id("org.jetbrains.intellij.platform.settings") version "2.20.1"
}

@Suppress("UnstableApiUsage")
dependencyResolutionManagement {
    repositories {
        mavenCentral()
        intellijPlatform {
            defaultRepositories()
        }
    }
}
