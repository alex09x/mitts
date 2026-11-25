/*
 * Mitts Build for JetBrains IDEs — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

package codes.prod.mitts

import com.intellij.execution.RunManager
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import org.jdom.Element

class MittsRunConfigurationTest : BasePlatformTestCase() {

    private fun newConfiguration(): MittsRunConfiguration =
        MittsConfigurationType.getInstance().configurationFactories.single()
            .createTemplateConfiguration(project) as MittsRunConfiguration

    fun testDefaultsToCheckInTheProjectDirectory() {
        val configuration = newConfiguration()
        assertEquals("check", configuration.command)
        assertEquals(project.basePath, configuration.effectiveDirectory())
    }

    fun testCommandAndDirectoryAreSavedAndRead() {
        val configuration = newConfiguration()
        configuration.command = "test -p a --release"
        configuration.workingDirectory = "/work/ws"
        val element = Element("configuration")
        configuration.writeExternal(element)

        val read = newConfiguration()
        read.readExternal(element)
        assertEquals("test -p a --release", read.command)
        assertEquals("/work/ws", read.effectiveDirectory())
    }

    fun testCommandLineSplitsTheCommandLikeAShell() {
        val line = MittsCommand.commandLine("""build -p "my pkg" --features a,b""", "/work/ws", "/opt/mitts")
        assertEquals("/opt/mitts", line.exePath)
        assertEquals(listOf("build", "-p", "my pkg", "--features", "a,b"), line.parametersList.list)
        assertEquals("/work/ws", line.workingDirectory.toString().replace('\\', '/').let {
            if (it.matches(Regex("^[A-Za-z]:.*"))) it.substring(2) else it
        })
    }

    fun testExecutableComesFromTheSettings() {
        val settings = MittsSettings.getInstance()
        val before = settings.executable
        try {
            settings.executable = "/custom/mitts"
            assertEquals("/custom/mitts", MittsCommand.commandLine("check", null).exePath)
            settings.executable = "  "
            assertEquals("mitts", MittsCommand.commandLine("check", null).exePath)
        } finally {
            settings.executable = before
        }
    }

    fun testActionsCreateOneTemporaryConfigurationPerCommand() {
        val first = MittsCommandAction.configuration(project, "clippy")
        val again = MittsCommandAction.configuration(project, "clippy")
        assertSame(first, again)
        assertTrue(first.isTemporary)
        assertEquals("clippy", (first.configuration as MittsRunConfiguration).command)
        assertSame(first, RunManager.getInstance(project).selectedConfiguration)
    }
}
